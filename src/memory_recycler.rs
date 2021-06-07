use core::alloc::Layout;
use std::{iter::repeat, marker::PhantomData, mem::{ManuallyDrop, MaybeUninit}, ops::{Deref, DerefMut, Range}, sync::{Arc, Mutex, mpsc::Sender}};

struct MemoryChunkRecycler {
    chunks: ManuallyDrop<Mutex<Vec<(Layout, Vec<*mut u8>)>>>
}

impl MemoryChunkRecycler {
    pub fn recycle(&self, chunk: RawMemoryChunk) {
        let guard = self.chunks.lock().unwrap();

        if let Some((_, found)) = guard.iter_mut().find(|a| a.0 == chunk.layout) {
            found.push(chunk.data);
        } else {
            guard.push((chunk.layout, vec![chunk.data]));
        }
    }
    pub fn get<T>(&self) -> MaybeUninit<MemoryChunk<T>> {
        let guard = self.chunks.lock().unwrap();
        let layout = Layout::new::<T>();

        let found= guard.iter_mut().enumerate().find(|(i, a)| a.0 == layout);

        let raw = match found {
            Some((i, (_, found))) if found.len() > 0 => {found.pop().unwrap()},
            _ => unsafe {
                std::alloc::alloc(layout)
            }
        };

        let chunk = MemoryChunk {
            raw: RawMemoryChunk {
                layout,
                data: raw,
            },
            recycler: (),
            _marker: (),
        };
    }
    pub fn cleanup(&self) {}
}

impl Drop for MemoryChunkRecycler {
    fn drop(&mut self) {
        let chunks = unsafe{
            ManuallyDrop::take(&mut self.chunks)
        };

        // interesting
        for (ptr, layout) in chunks.into_inner().unwrap().into_iter().flat_map(|(l, v)| v.into_iter().zip(repeat(l))) {
            unsafe{
                std::alloc::dealloc(ptr, layout)
            }
        }
    }
}

struct RawMemoryChunk {
    layout: Layout,
    data: *mut u8
}

impl Drop for RawMemoryChunk {
    fn drop(&mut self) {
        panic!("This leaks memory, it is an error to drop this struct.")
    }
}

struct MemoryChunk<T> {
    recycler: Arc<MemoryChunkRecycler>,
    raw: ManuallyDrop<RawMemoryChunk>,
    _marker: PhantomData<T>
}

impl<T> Drop for MemoryChunk<T> {
    fn drop(&mut self) {
        let raw = unsafe{
            ManuallyDrop::take(&mut self.raw)
        };

        self.recycler.recycle(raw)
    }
}

impl<T> Deref for MemoryChunk<T> {
    type Target = T;
    fn deref<'a>(&'a self) -> &'a Self::Target {
        unsafe {
            &*(self.raw.data as *const T)
        }
    }
} 

impl<T> DerefMut for MemoryChunk<T> {
    fn deref_mut<'a>(&'a mut self) -> &'a mut Self::Target {
        unsafe {
            &mut *(self.raw.data as *mut T)
        }
    }
} 