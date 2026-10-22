//! The host's art pack, served to the page through the app's own `dr-art` scheme (row L37): `protocol.handle`
//! in `src/main.ts`. The page never loads art from the network; the controller downloads it (L21),
//! checks it is an image, and registers each file here under its SHA-256. WebKitGTK asks for
//! `dr-art://bg/<id>`, WebView2 for `http://dr-art.localhost/bg/<id>`; both are answered.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use tauri::http::{Response, StatusCode, header};

pub const SCHEME: &str = "dr-art";

#[derive(Default)]
pub struct ArtStore {
    files: Mutex<HashMap<String, (PathBuf, &'static str)>>,
}

fn is_id(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The art id a request names, in either webview's form.
pub fn requested_id(uri: &tauri::http::Uri) -> Option<&str> {
    let path = uri.path().trim_start_matches('/');
    let id = match uri.host()? {
        "bg" => path,
        "dr-art.localhost" => path.strip_prefix("bg/")?,
        _ => return None,
    };
    is_id(id).then_some(id)
}

impl ArtStore {
    pub fn put(&self, id: &str, file: PathBuf, kind: &'static str) {
        if is_id(id) {
            self.files
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(id.to_owned(), (file, kind));
        }
    }

    #[cfg_attr(not(test), expect(dead_code, reason = "the tests use it"))]
    pub fn clear(&self) {
        self.files.lock().unwrap_or_else(|p| p.into_inner()).clear();
    }

    fn get(&self, id: &str) -> Option<(PathBuf, &'static str)> {
        self.files.lock().unwrap_or_else(|p| p.into_inner()).get(id).cloned()
    }

    /// The answer to one request: the image, or an empty 404.
    pub fn respond(&self, uri: &tauri::http::Uri) -> Response<Vec<u8>> {
        let found = requested_id(uri)
            .and_then(|id| self.get(id))
            .and_then(|(file, kind)| Some((std::fs::read(file).ok()?, kind)));
        let builder = Response::builder()
            .header("cache-control", "no-store")
            .header("x-content-type-options", "nosniff");
        let response = match found {
            Some((data, kind)) => builder
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, kind)
                .body(data),
            None => builder.status(StatusCode::NOT_FOUND).body(Vec::new()),
        };
        response.unwrap_or_else(|_| Response::new(Vec::new()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serves_registered_art_in_both_url_forms_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("dusk.jpg");
        std::fs::write(&file, [0xff, 0xd8, 0xff]).unwrap();
        let id = "a".repeat(64);
        let store = ArtStore::default();
        store.put(&id, file, "image/jpeg");
        for url in [format!("dr-art://bg/{id}"), format!("http://dr-art.localhost/bg/{id}")] {
            let response = store.respond(&url.parse().unwrap());
            assert_eq!(response.status(), StatusCode::OK, "{url}");
            assert_eq!(response.headers()[header::CONTENT_TYPE], "image/jpeg");
            assert_eq!(response.headers()["x-content-type-options"], "nosniff");
            assert_eq!(response.body(), &[0xff, 0xd8, 0xff]);
        }
        for url in [
            format!("dr-art://bg/{}", "b".repeat(64)),
            format!("dr-art://other/{id}"),
            "dr-art://bg/../../secret".into(),
            format!("http://evil.localhost/bg/{id}"),
        ] {
            assert_eq!(
                store.respond(&url.parse().unwrap()).status(),
                StatusCode::NOT_FOUND,
                "{url}"
            );
        }
        store.clear();
        assert_eq!(
            store.respond(&format!("dr-art://bg/{id}").parse().unwrap()).status(),
            StatusCode::NOT_FOUND
        );
    }
}
