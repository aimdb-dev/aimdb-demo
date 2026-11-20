//! Shared by the browser tests.
#![allow(dead_code)]

pub async fn sleep(ms: i32) {
    let p = js_sys::Promise::new(&mut |resolve, _| {
        web_sys::window()
            .unwrap()
            .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms)
            .unwrap();
    });
    wasm_bindgen_futures::JsFuture::from(p).await.unwrap();
}

/// Polls until `f` holds, or panics after `ms`.
pub async fn until(what: &str, ms: i32, mut f: impl FnMut() -> bool) {
    let mut waited = 0;
    while !f() {
        assert!(waited < ms, "timed out waiting for: {what}");
        sleep(10).await;
        waited += 10;
    }
}
