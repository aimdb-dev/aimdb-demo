//! Timers for the `/bus` client. `setTimeout` exists on windows and workers
//! alike.
use wasm_bindgen::JsCast;

pub async fn sleep_ms(ms: i32) {
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        let global: web_sys::WorkerGlobalScope = js_sys::global().unchecked_into();
        let _ = global.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}
