wit_bindgen::generate!({
    world: "handler",
});

struct MyHandler;

impl Guest for MyHandler {
    fn handle(method: String, path: String, body: String) -> String {
        format!(
            r#"{{"message":"Hello from Orca Wasm!","method":"{}","path":"{}","body_len":{}}}"#,
            method, path, body.len()
        )
    }
}

export!(MyHandler);
