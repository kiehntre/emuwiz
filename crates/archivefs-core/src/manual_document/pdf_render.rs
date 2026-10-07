//! Optional PDF rasterization seam. Hayro's internal allocations are not
//! bounded by its API, so production rendering is confined to a fresh Linux
//! process. The native launcher recognizes `HELPER_ARGUMENT` before GUI setup.
//! No PDF action, script, attachment, URL or external font loader is invoked.
//! Other platforms retain structural inspection and external-open fallback.
use super::{ManualDocument, ManualDocumentId, ManualDocumentKind, ManualLimits, ManualPageImage};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const HELPER_ARGUMENT: &str = "--emuwiz-render-manual-page-v1";
pub const MAX_PDF_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_RENDER_DIMENSION: u32 = 2048;
const MAX_OUTPUT_BYTES: u64 = 16 + MAX_RENDER_DIMENSION as u64 * MAX_RENDER_DIMENSION as u64 * 4;
const TIMEOUT: Duration = Duration::from_secs(15);
const MAGIC: &[u8; 8] = b"EMUPDF01";

pub const fn available() -> bool {
    cfg!(target_os = "linux")
}

#[derive(Serialize, Deserialize)]
struct Request {
    id: ManualDocumentId,
    page: usize,
    dimension: u32,
}

/// Rasterizes only the requested page using the application's own executable,
/// with a bounded IPC payload and a wall timeout. Never launches a shell.
pub fn render_page(
    document: &ManualDocument,
    page: usize,
    executable: &Path,
    dimension: u32,
) -> Result<ManualPageImage, String> {
    render_page_with_timeout(document, page, executable, dimension, TIMEOUT)
}

fn render_page_with_timeout(
    document: &ManualDocument,
    page: usize,
    executable: &Path,
    dimension: u32,
    timeout: Duration,
) -> Result<ManualPageImage, String> {
    if !available() {
        return Err(
            "Internal PDF rendering is unavailable on this platform. Open externally.".into(),
        );
    }
    if document.inspection().kind != ManualDocumentKind::Pdf {
        return Err("This is not a PDF document.".into());
    }
    document
        .check_unchanged()
        .map_err(|e| e.user_message().to_owned())?;
    if document.id().len > MAX_PDF_BYTES {
        return Err(
            "This PDF exceeds the internal renderer's 64 MiB limit. Open externally.".into(),
        );
    }
    let request = Request {
        id: document.id().clone(),
        page,
        dimension: dimension.clamp(1, MAX_RENDER_DIMENSION),
    };
    let payload = serde_json::to_vec(&request).map_err(|e| e.to_string())?;
    if payload.len() > 65536 {
        return Err("Document path is too long.".into());
    }
    let mut child = Command::new(executable)
        .arg(HELPER_ARGUMENT)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Could not start the PDF renderer: {e}. Open externally."))?;
    let deadline = Instant::now() + timeout;
    let output = child
        .stdout
        .take()
        .ok_or("PDF renderer has no output pipe.")?;
    // One bounded reader, joined before returning. Reading concurrently avoids
    // filling the pipe while the parent waits for process completion.
    let reader = match std::thread::Builder::new()
        .name("pdf-render-output".into())
        .spawn(move || super::read_bounded(output, MAX_OUTPUT_BYTES).map_err(|e| e.to_string()))
    {
        Ok(reader) => reader,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error.to_string());
        }
    };
    // Sending the request must also fit inside the wall deadline: a stuck
    // helper may never read stdin. Both IPC threads are joined after reaping.
    let input = child.stdin.take().expect("piped child stdin");
    let writer = match std::thread::Builder::new()
        .name("pdf-render-input".into())
        .spawn(move || {
            let mut input = input;
            input.write_all(&payload)
        }) {
        Ok(writer) => writer,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err(error.to_string());
        }
    };
    let status = loop {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let write_result = writer.join().map_err(|_| "PDF renderer input failed.")?;
    let bytes = reader.join().map_err(|_| "PDF renderer output failed.")??;
    if write_result.is_err() || !status.is_some_and(|status| status.success()) {
        return Err(
            "PDF rendering failed or exceeded its resource limits. Open externally.".into(),
        );
    }
    document
        .check_unchanged()
        .map_err(|e| e.user_message().to_owned())?;
    decode_reply(&bytes, request.dimension)
}

fn decode_reply(bytes: &[u8], dimension: u32) -> Result<ManualPageImage, String> {
    if bytes.len() < 16 || &bytes[..8] != MAGIC {
        return Err("Invalid PDF renderer reply.".into());
    }
    let width = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    let height = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
    if width == 0
        || height == 0
        || width > dimension
        || height > dimension
        || u64::from(width) * u64::from(height) * 4 != (bytes.len() - 16) as u64
    {
        return Err("PDF renderer exceeded its output bounds.".into());
    }
    Ok(ManualPageImage {
        width,
        height,
        rgba: bytes[16..].to_vec(),
    })
}

/// Entrypoint for a short-lived helper process. Call only before application
/// initialization: the limits intentionally apply to the entire process.
pub fn run_helper() -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        install_limits()?;
        let input =
            super::read_bounded(std::io::stdin().lock(), 65536).map_err(|e| e.to_string())?;
        let request: Request = serde_json::from_slice(&input).map_err(|e| e.to_string())?;
        let image = render_request(&request)?;
        let mut output = std::io::stdout().lock();
        output
            .write_all(MAGIC)
            .and_then(|_| output.write_all(&image.width.to_le_bytes()))
            .and_then(|_| output.write_all(&image.height.to_le_bytes()))
            .and_then(|_| output.write_all(&image.rgba))
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    Err("PDF rendering isolation is unavailable on this platform.".into())
}

#[cfg(target_os = "linux")]
fn install_limits() -> Result<(), String> {
    // A decompression bomb, excessive vector complexity or backend abort dies
    // inside this child. Hard limits cannot be raised by the renderer.
    for (resource, limit) in [
        (libc::RLIMIT_AS, 512 * 1024 * 1024),
        (libc::RLIMIT_CPU, 10),
        (libc::RLIMIT_CORE, 0),
    ] {
        let limits = libc::rlimit {
            rlim_cur: limit,
            rlim_max: limit,
        };
        // SAFETY: valid resource constants and pointer to a live rlimit.
        if unsafe { libc::setrlimit(resource, &limits) } != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn render_request(request: &Request) -> Result<ManualPageImage, String> {
    let limits = ManualLimits {
        max_file_bytes: MAX_PDF_BYTES,
        ..ManualLimits::default()
    };
    let document = ManualDocument::open(&request.id.path, &limits).map_err(|e| e.to_string())?;
    if document.id() != &request.id {
        return Err("Document changed before rendering.".into());
    }
    let index = document.pdf_page_index().map_err(|e| e.to_string())?;
    if request.page >= index.pages.len() {
        return Err("PDF page is out of range.".into());
    }
    let (mut source, metadata) =
        super::open_regular(&request.id.path).map_err(|e| e.to_string())?;
    if !request.id.still_matches(&metadata) {
        return Err("Document changed.".into());
    }
    let bytes = super::read_bounded(&mut source, MAX_PDF_BYTES).map_err(|e| e.to_string())?;
    document.check_unchanged().map_err(|e| e.to_string())?;
    let pdf = hayro::hayro_syntax::Pdf::new(bytes).map_err(|e| format!("Invalid PDF: {e:?}"))?;
    if pdf.pages().len() != index.pages.len() {
        return Err("PDF page counts disagree.".into());
    }
    let page = &pdf.pages()[request.page];
    let (width, height) = page.render_dimensions();
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return Err("Invalid PDF page dimensions.".into());
    }
    let dimension = request.dimension.clamp(1, MAX_RENDER_DIMENSION);
    let scale = (dimension as f32 / width.max(height)).min(2.0);
    if width * scale < 1.0 || height * scale < 1.0 {
        return Err("PDF page aspect ratio is unsupported.".into());
    }
    let pixmap = hayro::render(
        page,
        &hayro::RenderCache::new(),
        &hayro::hayro_interpret::InterpreterSettings::default(),
        &hayro::RenderSettings::default(),
        &hayro::PixmapSettings {
            x_scale: scale,
            y_scale: scale,
            bg_color: hayro::vello_cpu::color::palette::css::WHITE,
        },
    );
    document.check_unchanged().map_err(|e| e.to_string())?;
    let image = ManualPageImage {
        width: u32::from(pixmap.width()),
        height: u32::from(pixmap.height()),
        rgba: pixmap.take_rgba8(hayro::vello_cpu::peniko::ImageAlphaType::Alpha),
    };
    if image.width == 0 || image.height == 0 || image.width > dimension || image.height > dimension
    {
        return Err("PDF output dimensions exceed their bounds.".into());
    }
    Ok(image)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::fs;

    fn pdf(width: u32, height: u32) -> Vec<u8> {
        let content = "1 0 0 rg 0 0 10 10 re f";
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Count 2 /Kids [3 0 R 4 0 R] >>".to_owned(),
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {width} {height}] /Resources << >> /Contents 5 0 R >>"
            ),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 20 30] /Resources << >> >>".to_owned(),
            format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            ),
        ];
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, object) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
        }
        let xref = bytes.len();
        bytes.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
        for offset in offsets {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(
            format!("trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
        );
        bytes
    }

    fn fixture(bytes: &[u8]) -> (tempfile::TempDir, ManualDocument) {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("manual.pdf");
        fs::write(&path, bytes).unwrap();
        let doc = ManualDocument::open(&path, &ManualLimits::default()).unwrap();
        (root, doc)
    }

    #[test]
    fn page_count_and_requested_page_render_without_source_writes() {
        let before = pdf(20, 30);
        let (_root, doc) = fixture(&before);
        assert_eq!(doc.pdf_page_index().unwrap().pages.len(), 2);
        let request = Request {
            id: doc.id().clone(),
            page: 0,
            dimension: 100,
        };
        let page = render_request(&request).unwrap();
        assert_eq!((page.width, page.height), (40, 60));
        assert!(
            page.rgba
                .chunks_exact(4)
                .any(|pixel| pixel == [255, 0, 0, 255])
        );
        let blank = render_request(&Request { page: 1, ..request }).unwrap();
        assert!(
            blank
                .rgba
                .chunks_exact(4)
                .all(|pixel| pixel == [255, 255, 255, 255])
        );
        assert_eq!(fs::read(&doc.id().path).unwrap(), before);
        assert_eq!(
            fs::read_dir(doc.id().path.parent().unwrap())
                .unwrap()
                .count(),
            1
        );
    }

    #[test]
    fn huge_page_dimensions_are_downscaled_and_output_is_bounded() {
        let (_root, doc) = fixture(&pdf(500_000, 500_000));
        let page = render_request(&Request {
            id: doc.id().clone(),
            page: 0,
            dimension: 64,
        })
        .unwrap();
        assert_eq!((page.width, page.height), (64, 64));
        assert_eq!(page.rgba.len(), 64 * 64 * 4);
    }

    #[test]
    fn corrupt_pdf_page_range_and_changed_source_fail_closed() {
        let (_root, doc) = fixture(&pdf(20, 30));
        let request = Request {
            id: doc.id().clone(),
            page: 2,
            dimension: 100,
        };
        assert!(
            render_request(&request)
                .unwrap_err()
                .contains("out of range")
        );
        fs::write(&doc.id().path, b"%PDF-1.7\ncorrupt").unwrap();
        assert!(render_request(&Request { page: 0, ..request }).is_err());
    }

    #[test]
    fn renderer_reply_refuses_invalid_sizes_lengths_and_magic() {
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&[255; 16]);
        assert_eq!(decode_reply(&bytes, 2).unwrap().rgba.len(), 16);
        assert!(decode_reply(&bytes, 1).is_err());
        bytes.push(0);
        assert!(decode_reply(&bytes, 2).is_err());
        assert!(decode_reply(b"wrong magic", 2048).is_err());
    }

    #[test]
    fn malformed_backend_input_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("manual.pdf");
        fs::write(&path, b"%PDF-1.7\ncorrupt").unwrap();
        let id = ManualDocumentId::capture(&path, &fs::metadata(&path).unwrap());
        assert!(
            render_request(&Request {
                id,
                page: 0,
                dimension: 128
            })
            .is_err()
        );
    }

    #[test]
    fn failed_helper_keeps_external_fallback_and_does_not_change_source() {
        let before = pdf(20, 30);
        let (root, doc) = fixture(&before);
        let helper = root.path().join("helper");
        fs::write(&helper, b"#!/bin/sh\nexit 1\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            render_page(&doc, 0, &helper, 128)
                .unwrap_err()
                .contains("externally")
        );
        assert_eq!(fs::read(&doc.id().path).unwrap(), before);
    }
    #[test]
    fn stalled_helper_is_killed_and_reaped_with_bounded_output() {
        let (root, doc) = fixture(&pdf(20, 30));
        let helper = root.path().join("helper");
        fs::write(&helper, b"#!/bin/sh\nexec /bin/sleep 5\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
        let started = Instant::now();
        assert!(
            render_page_with_timeout(&doc, 0, &helper, 128, Duration::from_millis(50)).is_err()
        );
        assert!(started.elapsed() < Duration::from_secs(3));
    }
}
