//! HTTP over the WinRT HttpClient: the update check's and download's GETs
//! and the run log upload's POST.

use windows::Web::Http::HttpClient;
use windows::core::{HSTRING, h};

/// "warden/X.Y.Z", sent as the User-Agent.
const USER_AGENT: &str = concat!("warden/", env!("CARGO_PKG_VERSION"));

/// A client that sends the User-Agent and `headers` with each request.
fn client(headers: &[(&str, &str)]) -> windows::core::Result<HttpClient> {
    let client = HttpClient::new()?;
    let defaults = client.DefaultRequestHeaders()?;
    defaults.TryAppendWithoutValidation(h!("User-Agent"), &HSTRING::from(USER_AGENT))?;
    for (name, value) in headers {
        defaults.TryAppendWithoutValidation(&HSTRING::from(*name), &HSTRING::from(*value))?;
    }
    Ok(client)
}

/// The body of a GET of `url`. HttpClient follows redirects, such as the
/// release asset's to objects.githubusercontent.com.
pub fn get(url: &str, accept: &str) -> Result<Vec<u8>, String> {
    use windows::Foundation::Uri;
    use windows::Storage::Streams::DataReader;
    let fetch = || -> windows::core::Result<Result<Vec<u8>, String>> {
        let client = client(&[("Accept", accept)])?;
        let response = client.GetAsync(&Uri::CreateUri(&HSTRING::from(url))?)?.join()?;
        if !response.IsSuccessStatusCode()? {
            return Ok(Err(format!("HTTP {}", response.StatusCode()?.0)));
        }
        let buffer = response.Content()?.ReadAsBufferAsync()?.join()?;
        let mut bytes = vec![0; buffer.Length()? as usize];
        DataReader::FromBuffer(&buffer)?.ReadBytes(&mut bytes)?;
        Ok(Ok(bytes))
    };
    fetch().map_err(|e| e.message()).and_then(|r| r).map_err(|e| format!("{url}: {e}"))
}

/// POSTs `body` as `content_type` to `url` with `headers`; the status and
/// the response's text for any status. Err only when no response came.
pub fn post(url: &str, content_type: &str, headers: &[(&str, &str)], body: &[u8]) -> Result<(u16, String), String> {
    use windows::Foundation::Uri;
    use windows::Storage::Streams::DataWriter;
    use windows::Web::Http::HttpBufferContent;
    use windows::Web::Http::Headers::HttpMediaTypeHeaderValue;
    let send = || -> windows::core::Result<(u16, String)> {
        let client = client(headers)?;
        let writer = DataWriter::new()?;
        writer.WriteBytes(body)?;
        let content = HttpBufferContent::CreateFromBuffer(&writer.DetachBuffer()?)?;
        content.Headers()?.SetContentType(&HttpMediaTypeHeaderValue::Create(&HSTRING::from(content_type))?)?;
        let response = client.PostAsync(&Uri::CreateUri(&HSTRING::from(url))?, &content)?.join()?;
        let status = response.StatusCode()?.0 as u16;
        let text = response.Content()?.ReadAsStringAsync()?.join()?;
        Ok((status, text.to_string()))
    };
    send().map_err(|e| format!("{url}: {}", e.message()))
}
