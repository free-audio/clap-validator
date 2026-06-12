mod config;
mod log;
mod panic;
mod print;
pub mod sandbox;
pub mod tracing;

pub use config::*;
pub use log::*;
pub use panic::*;
pub use print::*;

/// A temporary directory used by the validator. This is cleared when launching the validator.
pub fn validator_temp_dir() -> std::path::PathBuf {
    /// [`std::env::temp_dir`], but taking `XDG_RUNTIME_DIR` on Linux into account.
    fn temp_dir() -> std::path::PathBuf {
        #[cfg(all(unix, not(target_os = "macos")))]
        if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR").map(std::path::PathBuf::from)
            && dir.is_dir()
        {
            return dir;
        }

        std::env::temp_dir()
    }

    temp_dir().join("clap-validator")
}

impl<T: ?Sized> IteratorExt for T where T: Iterator {}
pub trait IteratorExt: Iterator {
    fn parallel_fork_join<R: Send>(
        self,
        workers: Option<usize>,
        fork: impl Fn(Self::Item) -> R + Send + Sync,
        mut join: impl FnMut(R),
    ) where
        Self: Sized + Send,
        Self::Item: Send,
    {
        use std::sync::Mutex;

        let workers = match workers {
            Some(n) => n,
            None => std::thread::available_parallelism().map_or(1, |n| n.get()),
        };

        if workers <= 1 {
            return self.map(fork).for_each(join);
        }

        let inputs = Mutex::new(self.fuse());
        let (output_tx, output_rx) = std::sync::mpsc::channel();

        std::thread::scope(|scope| {
            for _ in 0..workers {
                let map = &fork;
                let inputs = &inputs;
                let output_tx = output_tx.clone();
                scope.spawn(move || {
                    loop {
                        let item = inputs.lock().unwrap().next();
                        let Some(item) = item else { break };
                        output_tx.send(map(item)).unwrap();
                    }
                });
            }

            drop(output_tx);
            while let Ok(item) = output_rx.recv() {
                join(item);
            }
        });
    }

    /// Map this iterator in parallel using the given number of worker threads. Unordered.
    fn parallel_map<R: Send>(
        self,
        workers: Option<usize>,
        f: impl Fn(Self::Item) -> R + Send + Sync,
    ) -> impl Iterator<Item = R>
    where
        Self: Sized + Send,
        Self::Item: Send,
    {
        let mut vec = Vec::with_capacity(self.size_hint().0);
        self.parallel_fork_join(workers, f, |item| vec.push(item));
        vec.into_iter()
    }
}
