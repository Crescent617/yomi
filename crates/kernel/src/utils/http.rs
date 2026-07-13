use std::sync::OnceLock;
use std::time::Duration;

const REQUEST_TIMEOUT: Duration = Duration::from_mins(1);
const MAX_REDIRECTS: usize = 5;

/// Shared HTTP client for general-purpose web requests.
static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

/// Returns the shared HTTP client with connection pooling.
pub fn client() -> &'static reqwest::Client {
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS))
            .build()
            .expect("failed to build shared HTTP client")
    })
}
