use std::{
    any::Any,
    marker::PhantomData,
    ops::DerefMut,
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
};

pub struct WorkerPoisoned;
pub enum Poll<T> {
    Pending,
    Ready(T),
    Finished,
}
pub trait Task: Send + 'static {
    type Output: Send;
    fn process(self) -> Self::Output;
}

// trait InternalTask: Send + 'static {
//     fn process(self) -> Box<dyn Any>;
// }

// impl<O: Send, T: Task<Output=O>> InternalTask for T {
//     fn process(self) -> Box<dyn Any> {
//         let output = Task::process(self);
//         Box::new(output)
//     }
// }

struct TypeEraser<T: Task>(T);

// impl<T: Task> Task for TypeEraser<T> {
//     type Output = Box<dyn Any + Send>;
//     fn process(self) -> Self::Output {
//         let output = <T as Task>::process(self.0);

//         Box::new(output)
//     }
// }
// struct _TypeEraser<T: Task>(T);

trait TypeErasedTask: Send + 'static {
    fn process(self: Box<Self>) -> Box<dyn Any + Send>;
}

impl<T: Task> TypeErasedTask for TypeEraser<T> {
    fn process(self: Box<Self>) -> Box<dyn Any + Send> {
        let output = <T as Task>::process(self.0);

        Box::new(output)
    }
}

enum Work {
    Work(Box<dyn TypeErasedTask>),
    Ready(Box<dyn Any + Send>),
}
pub struct FinishedMaybe<T: Send + 'static> {
    _marker: PhantomData<T>,
    // the mutex could be replaced with an atomic cell
    work: Option<Arc<Mutex<Work>>>,
}

unsafe impl<T: Send + 'static> Send for FinishedMaybe<T> {}

impl<T: Send + 'static> FinishedMaybe<T> {
    pub fn poll(&mut self) -> Result<Poll<T>, WorkerPoisoned> {
        // the task was already processed and retrieved
        if self.work.is_none() {
            return Ok(Poll::Finished);
        }

        let work_arc = self.work.as_ref().unwrap();

        // the worker still has a reference to the data so it is waiting to be processed or processing
        if Arc::strong_count(&work_arc) != 1 {
            return Ok(Poll::Pending);
        }

        let lock = match Arc::try_unwrap(self.work.take().unwrap()) {
            Ok(lock) => lock,
            Err(_) => unreachable!("Arc unwrap failed even though the strong count is 1"),
        };

        let ready = match lock.into_inner() {
            Err(_) => return Err(WorkerPoisoned),
            Ok(Work::Ready(any)) => any.downcast::<T>().unwrap(),
            Ok(Work::Work(_)) => unreachable!("Worker didn't process task"),
        };

        Ok(Poll::Ready(*ready))
    }
}

struct Worker {
    thread: JoinHandle<()>,
    queue: Arc<Mutex<Vec<Arc<Mutex<Work>>>>>,
}

impl Worker {
    fn new() -> Self {
        let queue: Arc<Mutex<Vec<Arc<Mutex<Work>>>>> = Arc::new(Mutex::new(Vec::new()));

        let queue_clone = queue.clone();
        let thread = thread::Builder::new()
            .name("Simple worker".to_string())
            .spawn(move || loop {
                let next_work = {
                    let mut guard = queue_clone.lock().unwrap();
                    guard.pop()
                };

                // the workpool is empty, park and be unparked when new work is pushed onto the queue
                if next_work.is_none() {
                    thread::park();
                    continue;
                }

                let next_work = next_work.unwrap();
                let mut guard = next_work.lock().unwrap();

                let output = match &mut *guard {
                    // reached
                    Work::Ready(_) => unreachable!("Worker encountered an already processed task"),
                    Work::Work(task) => {
                        let task = unsafe { std::ptr::read(task) };

                        Work::Ready(task.process())
                    }
                };

                let old = std::mem::replace(guard.deref_mut(), output);
                // the previous value was ptr::read so it must be unsured that the original memory isn't dropped
                std::mem::forget(old);
            })
            .unwrap();

        Self { thread, queue }
    }
    fn add_work<T: Send + 'static>(
        &mut self,
        task: impl Task<Output = T>,
    ) -> Result<FinishedMaybe<T>, WorkerPoisoned> {
        let mut guard = self.queue.lock().map_err(|_| WorkerPoisoned)?;

        let work = Work::Work(Box::new(TypeEraser(task)));
        let arc_work = Arc::new(Mutex::new(work));

        guard.push(arc_work.clone());
        drop(guard);

        self.thread.thread().unpark();

        Ok(FinishedMaybe {
            _marker: PhantomData,
            work: Some(arc_work),
        })
    }
}

impl<O: Send + 'static, F: FnOnce() -> O + Send + 'static> Task for F {
    type Output = O;
    fn process(self) -> Self::Output {
        (self)()
    }
}
