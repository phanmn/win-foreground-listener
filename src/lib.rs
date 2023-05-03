use neon::prelude::*;
use once_cell::sync::OnceCell;
use std::sync::Arc;
use std::{
    cell::RefCell,
    io,
    num::{NonZeroU32, NonZeroUsize},
    ptr::{self, NonNull},
};
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;

use winapi::{
    shared::windef::HWND,
    um::{
        errhandlingapi::{GetLastError, SetLastError},
        winuser::{GetWindowTextLengthW, GetWindowTextW},
    },
};
use wineventhook::{raw_event, AccessibleObjectId, EventFilter, WindowEventHook};

type BoxedListener = JsBox<RefCell<WindowForegroundListener>>;

struct WindowForegroundListener {
    join_handle: Option<JoinHandle<()>>,
}

impl Finalize for WindowForegroundListener {}

impl WindowForegroundListener {
    fn new() -> Self {
        Self { join_handle: None }
    }

    fn start(&mut self, rt: &Runtime, pid: u32, js_callback: JsCallback) {
        self.stop();

        let join_handle = listen(rt, pid, js_callback);

        self.join_handle = Some(join_handle);
    }

    fn stop(&mut self) {
        match &self.join_handle {
            Some(join_handle) => {
                join_handle.abort();
                self.join_handle = None;
            }

            _ => (),
        }
    }
}

impl WindowForegroundListener {
    fn js_new(mut cx: FunctionContext) -> JsResult<BoxedListener> {
        let listener = WindowForegroundListener::new();

        Ok(cx.boxed(RefCell::new(listener)))
    }

    fn js_start(mut cx: FunctionContext) -> JsResult<JsUndefined> {
        let rt = runtime(&mut cx)?;
        let pid = cx.argument::<JsNumber>(0)?.value(&mut cx) as u32;
        let js_callback = JsCallback {
            channel: cx.channel(),
            callback: Arc::new(cx.argument::<JsFunction>(1)?.root(&mut cx)),
        };

        let listener = cx.this().downcast_or_throw::<BoxedListener, _>(&mut cx)?;
        let mut listener = listener.borrow_mut();
        listener.start(rt, pid, js_callback);

        Ok(cx.undefined())
    }

    fn js_stop(mut cx: FunctionContext) -> JsResult<JsUndefined> {
        let listener = cx.this().downcast_or_throw::<BoxedListener, _>(&mut cx)?;
        let mut listener = listener.borrow_mut();

        listener.stop();
        Ok(cx.undefined())
    }
}

#[neon::main]
fn main(mut cx: ModuleContext) -> NeonResult<()> {
    cx.export_function("listenerNew", WindowForegroundListener::js_new)?;
    cx.export_function("listenerStart", WindowForegroundListener::js_start)?;
    cx.export_function("listenerStop", WindowForegroundListener::js_stop)?;
    Ok(())
}

// Return a global tokio runtime or create one if it doesn't exist.
// Throws a JavaScript exception if the `Runtime` fails to create.
fn runtime<'a, C: Context<'a>>(cx: &mut C) -> NeonResult<&'static Runtime> {
    static RUNTIME: OnceCell<Runtime> = OnceCell::new();

    RUNTIME.get_or_try_init(|| Runtime::new().or_else(|err| cx.throw_error(err.to_string())))
}

fn listen(rt: &Runtime, pid: u32, js_callback: JsCallback) -> JoinHandle<()> {
    return rt.spawn(async move {
        let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
        let filter = EventFilter::default().event(raw_event::SYSTEM_FOREGROUND);
        let filter = match NonZeroU32::new(pid) {
            Some(pid) => filter.process(pid),
            _ => filter,
        };

        let hook = WindowEventHook::hook(filter, event_tx).await.unwrap();

        while let Some(event) = event_rx.recv().await {
            if event.object_type() == AccessibleObjectId::Window {
                let hwnd = format!(
                    "{}",
                    (event
                        .window_handle()
                        .map_or_else(ptr::null_mut, NonNull::as_ptr)) as isize
                );

                let result = js_callback.call(hwnd).await;
                // let title = get_window_text(
                //     event
                //         .window_handle()
                //         .map_or_else(ptr::null_mut, NonNull::as_ptr),
                // )
                // .unwrap();
                // let result = match title {
                //     Some(v) => js_callback.call(v).await,
                //     None => js_callback.call(String::new()).await,
                // };

                match result {
                    Err(err) => println!("Failed to call JavaScript: {:?}", err),
                    _ => (),
                }

                ()
            }
        }

        hook.unhook().await.unwrap();
    });
}

// fn get_window_text_length(window: HWND) -> io::Result<Option<NonZeroUsize>> {
//     unsafe { SetLastError(0) };
//     let result = unsafe { GetWindowTextLengthW(window) };
//     if result == 0 && unsafe { GetLastError() } != 0 {
//         Err(io::Error::last_os_error())
//     } else {
//         Ok(NonZeroUsize::new(result as usize))
//     }
// }

// fn get_window_text(window: HWND) -> io::Result<Option<String>> {
//     let text_len = if let Some(length) = get_window_text_length(window)? {
//         length.get()
//     } else {
//         return Ok(None);
//     };

//     let mut text = Vec::with_capacity(text_len + 1); // +1 for null terminator
//     let result = unsafe { GetWindowTextW(window, text.as_mut_ptr(), text.capacity() as i32) };
//     if result != 0 {
//         unsafe { text.set_len(text_len) };
//         let text = String::from_utf16_lossy(&text);
//         Ok(Some(text))
//     } else {
//         Err(io::Error::last_os_error())
//     }
// }

// https://github.com/neon-bindings/neon/issues/848
// https://github.dev/owenthereal/neon-tonic-example/blob/master/src/lib.rs
pub struct JsCallback {
    channel: Channel,
    callback: Arc<Root<JsFunction>>,
}

impl JsCallback {
    pub async fn call(
        &self,
        name: String,
    ) -> Result<String, tokio::sync::oneshot::error::RecvError> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let callback = self.callback.clone();
        let _ = self.channel.try_send(move |mut cx| {
            let this = cx.undefined();
            let arg = cx.string(name);

            let value = callback
                .to_inner(&mut cx)
                .call(&mut cx, this, vec![arg.upcast()])?
                .downcast_or_throw::<JsString, _>(&mut cx)?
                .value(&mut cx);

            let _ = tx.send(value);

            Ok(())
        });

        rx.await
        // rx.await?
        // rx.await
        //     .map_err(|err: tokio::sync::oneshot::error::RecvError| format!("Failed to call JavaScript: {:?}", err)).
    }
}
