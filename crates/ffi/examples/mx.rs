// SPDX-License-Identifier: MIT OR Apache-2.0
use forge_ffi::{Forge, Key, KeyInput};
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::sleep;
use std::time::Duration;

/// Drive an async getter to its value. The getters wait for the editor
/// thread inside their first poll, so this never parks.
fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    struct ThreadWaker(std::thread::Thread);
    impl Wake for ThreadWaker {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut fut = std::pin::pin!(fut);
    loop {
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => std::thread::park(),
        }
    }
}

fn main() {
    let forge = Forge::new(vec![], false).expect("start");
    sleep(Duration::from_millis(300));
    let f0 = block_on(forge.frame(100, 30));
    println!(
        "before: completions={} echo={:?}",
        f0.completions.len(),
        f0.echo
    );
    forge.key(KeyInput {
        key: Key::Char { c: "x".into() },
        control: false,
        meta: true,
        shift: false,
    });
    for i in 0..20 {
        let f = block_on(forge.frame(100, 30));
        println!(
            "{i}: completions={} echo={:?} cursor=({},{}) windows={}",
            f.completions.len(),
            f.echo,
            f.cursor_x,
            f.cursor_y,
            f.windows.len()
        );
        sleep(Duration::from_millis(100));
    }
}
