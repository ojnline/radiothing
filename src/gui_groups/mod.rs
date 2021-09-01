pub mod decode_group;
pub mod device_group;
pub mod habhub_group;
pub mod output_group;
pub mod receive_group;

use crate::worker::worker_manager::DeviceError;

// crash on BadState, ignore WorkerPoisoned because it will be handled in the next iteration
// previously the code ws just unwrapping the result which enabled a race condition when the worker thread has just closed
// obviously on a bad state we want to crash regardless but a panic is a much nicer error
pub fn handle_send_result(result: Result<(), DeviceError>) {
    match result {
        Err(DeviceError::BadState) => {
            panic!("Application is in the wrong state, this is a fatal error, shutting down");
        }
        Err(DeviceError::WorkerPoisoned) | Ok(()) => {}
    }
}
