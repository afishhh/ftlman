use anyhow::{Result, bail};
use ureq::Response;

pub fn check_respones_status(res: &ureq::Response) -> Result<()> {
    if res.status() != 200 {
        bail!(
            "Received unsuccessful response code: {} {}",
            res.status(),
            res.status_text()
        );
    }

    Ok(())
}

pub fn download_body_with_progress(
    response: Response,
    mut on_progress: impl FnMut(u64, Option<u64>),
) -> Result<Vec<u8>> {
    let is_chunked = response
        .header("Transfer-Encoding")
        .is_some_and(|x| x.eq_ignore_ascii_case("chunked"));

    let content_length = response
        .header("Content-Length")
        .filter(|_| !is_chunked)
        .and_then(|x| x.parse::<u64>().ok());

    let mut reader = response.into_reader();

    const BUFFER_SIZE: usize = 4096;
    let mut out = vec![0; BUFFER_SIZE];
    loop {
        let len = out.len();
        let nread = reader.read(&mut out[(len - BUFFER_SIZE)..])?;
        if nread == 0 {
            out.resize(out.len() - BUFFER_SIZE, 0);
            break;
        } else {
            out.extend(std::iter::repeat_n(0, nread));
            on_progress((out.len() - BUFFER_SIZE) as u64, content_length);
        }
    }

    Ok(out)
}
