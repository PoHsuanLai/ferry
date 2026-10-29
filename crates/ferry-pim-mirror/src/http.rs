//! The one request the JSON sources (Graph, Google) make: a GET that asks for JSON.

use porter_http::http::{Method, Response};
use porter_http::{Http, HttpError, request};
use porter_ids::WebUrl;

/// `GET url` asking for JSON, with a `Prefer` header when `prefer` is given.
pub async fn get<H: Http>(
    http: &H,
    url: &str,
    prefer: Option<&str>,
) -> Result<Response<Vec<u8>>, HttpError> {
    let url = WebUrl::parse(url).map_err(|_| HttpError::Unreachable)?;
    let mut builder = request(Method::GET, &url).header("Accept", "application/json");
    if let Some(prefer) = prefer {
        builder = builder.header("Prefer", prefer);
    }
    http.send(builder.body(Vec::new())?).await
}
