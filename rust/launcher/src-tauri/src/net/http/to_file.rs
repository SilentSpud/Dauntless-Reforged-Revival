//! `requestToFile` (Reforged's `src/main/http.ts`): one GET streamed to a file, with the key headers, an
//! idle timeout and a size cap, for the runtime's two files (RD22: DR's `net/downloader.rs` is not
//! brought in). A file of its own so RA7 fills it while RA5 changes the rest of `net/http.rs`; RA7 may
//! re-export it from there.

use std::path::Path;

use super::Endpoint;
use crate::stub::{NotImplemented, not_implemented};

/// Streams `path` on `endpoint` into `target`; at most `max_bytes`.
pub async fn request_to_file(
    _endpoint: &Endpoint,
    _path: &str,
    _headers: &[(&str, &str)],
    _target: &Path,
    _max_bytes: u64,
) -> Result<u64, NotImplemented> {
    Err(not_implemented("RA7"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_stub_names_its_row() {
        let endpoint = Endpoint {
            host: "127.0.0.1".into(),
            port: 61000,
            pin: None,
        };
        let copied = request_to_file(&endpoint, "/x", &[], Path::new("x"), 1).await;
        assert_eq!(copied, Err(not_implemented("RA7")));
    }
}
