use chrono::Utc;
use narayani_core::{DatabaseResource, ResourceName};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_UPLOAD_DIR: &str = "uploads";
const DEFAULT_MAX_FILE_SIZE_BYTES: usize = 25 * 1024 * 1024;
const DEFAULT_PUBLIC_BASE_PATH: &str = "/files";
const MAX_SANITIZED_FILENAME_LEN: usize = 120;

static FILE_SEQ: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone)]
pub struct FileUploadConfig {
    pub upload_dir: PathBuf,
    pub max_file_size_bytes: usize,
    pub public_base_path: String,
}

impl Default for FileUploadConfig {
    fn default() -> Self {
        Self {
            upload_dir: PathBuf::from(DEFAULT_UPLOAD_DIR),
            max_file_size_bytes: DEFAULT_MAX_FILE_SIZE_BYTES,
            public_base_path: DEFAULT_PUBLIC_BASE_PATH.to_string(),
        }
    }
}

impl FileUploadConfig {
    pub fn new(upload_dir: impl Into<PathBuf>) -> Self {
        Self {
            upload_dir: upload_dir.into(),
            ..Self::default()
        }
    }

    pub fn from_env() -> Self {
        let upload_dir = std::env::var("UPLOAD_DIR")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_UPLOAD_DIR));

        let max_file_size_bytes = std::env::var("MAX_UPLOAD_SIZE_BYTES")
            .ok()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(DEFAULT_MAX_FILE_SIZE_BYTES);

        let public_base_path = std::env::var("PUBLIC_FILES_BASE_PATH")
            .ok()
            .map(|s| s.trim().trim_end_matches('/').to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_PUBLIC_BASE_PATH.to_string());

        Self {
            upload_dir,
            max_file_size_bytes,
            public_base_path,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileMetadata {
    pub original_name: String,
    pub stored_name: String,
    pub content_type: String,
    pub size_bytes: usize,
    pub url: String,
    pub disk_path: String,
    pub uploaded_at: String,
    pub uploaded_by: Option<String>,
}

impl FileMetadata {
    pub fn to_file_resource(&self) -> DatabaseResource {
        DatabaseResource {
            id: None,
            resource_name: Some(ResourceName::FileResource),
            inner_content: Some(serde_json::to_value(self).unwrap_or_default()),
            external_content: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedUpload {
    pub filename: String,
    pub content_type: Option<String>,
    pub data: Vec<u8>,
}

#[derive(Debug)]
pub enum FileUploadError {
    EmptyFile,
    FileTooLarge { size_bytes: usize, max_bytes: usize },
    InvalidMultipart(String),
    InvalidFilename(String),
    NotFound(String),
    Io(std::io::Error),
}

impl fmt::Display for FileUploadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyFile => write!(f, "Uploaded file payload is empty"),
            Self::FileTooLarge {
                size_bytes,
                max_bytes,
            } => write!(
                f,
                "Uploaded file size ({size_bytes} bytes) exceeds limit ({max_bytes} bytes)"
            ),
            Self::InvalidMultipart(msg) => write!(f, "Invalid multipart payload: {msg}"),
            Self::InvalidFilename(name) => write!(f, "Invalid file name: '{name}'"),
            Self::NotFound(name) => write!(f, "File not found: '{name}'"),
            Self::Io(err) => write!(f, "File I/O error: {err}"),
        }
    }
}

impl std::error::Error for FileUploadError {}

impl From<std::io::Error> for FileUploadError {
    fn from(err: std::io::Error) -> Self {
        match err.kind() {
            std::io::ErrorKind::NotFound => Self::NotFound(err.to_string()),
            _ => Self::Io(err),
        }
    }
}

/// Validates that a stored filename refers to a single flat file inside the upload directory.
pub fn is_safe_stored_filename(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && !name.starts_with('.')
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains('\0')
        && !name.contains("..")
}

/// Sanitizes an arbitrary client-supplied filename into a safe ASCII filename.
pub fn sanitize_filename(name: &str) -> String {
    let base = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .trim();

    let mut cleaned = String::with_capacity(base.len());
    for ch in base.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
            cleaned.push(ch);
        } else {
            cleaned.push('_');
        }
    }

    let trimmed = cleaned.trim_start_matches(['.', '_']);
    let safe = if trimmed.is_empty() {
        "file.bin".to_string()
    } else {
        trimmed.to_string()
    };

    if safe.len() <= MAX_SANITIZED_FILENAME_LEN {
        return safe;
    }

    if let Some((stem, ext)) = safe.rsplit_once('.') {
        if !ext.is_empty() && ext.len() <= 16 {
            let max_stem = MAX_SANITIZED_FILENAME_LEN.saturating_sub(ext.len() + 1).max(1);
            let truncated_stem: String = stem.chars().take(max_stem).collect();
            return format!("{truncated_stem}.{ext}");
        }
    }

    safe.chars().take(MAX_SANITIZED_FILENAME_LEN).collect()
}

pub fn detect_content_type_from_ext(filename: &str) -> &'static str {
    let ext = Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());

    match ext.as_deref() {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("bmp") => "image/bmp",
        Some("ico") => "image/x-icon",
        Some("pdf") => "application/pdf",
        Some("txt") | Some("log") => "text/plain; charset=utf-8",
        Some("csv") => "text/csv; charset=utf-8",
        Some("json") => "application/json",
        Some("xml") => "application/xml",
        Some("html") | Some("htm") => "text/html; charset=utf-8",
        Some("mp4") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        Some("zip") => "application/zip",
        Some("doc") => "application/msword",
        Some("docx") => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        Some("xls") => "application/vnd.ms-excel",
        Some("xlsx") => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        _ => "application/octet-stream",
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn extract_boundary(content_type: &str) -> Option<String> {
    for param in content_type.split(';').skip(1) {
        let trimmed = param.trim();
        if let Some((k, v)) = trimmed.split_once('=') {
            if k.trim().eq_ignore_ascii_case("boundary") {
                let boundary = v.trim().trim_matches('"').trim();
                if !boundary.is_empty() {
                    return Some(boundary.to_string());
                }
            }
        }
    }
    None
}

fn parse_disposition_param(header_val: &str, key: &str) -> Option<String> {
    for part in header_val.split(';').skip(1) {
        let trimmed = part.trim();
        if let Some((k, v)) = trimmed.split_once('=') {
            if k.trim().eq_ignore_ascii_case(key) {
                let val = v.trim().trim_matches('"').to_string();
                return Some(val);
            }
        }
    }
    None
}

/// Parses either a `multipart/form-data` request body or a raw binary upload body.
pub fn parse_upload_payload(
    content_type_header: Option<&str>,
    fallback_filename: Option<&str>,
    body: &[u8],
) -> Result<ParsedUpload, FileUploadError> {
    if body.is_empty() {
        return Err(FileUploadError::EmptyFile);
    }

    let is_multipart = content_type_header
        .map(|ct| {
            ct.trim()
                .to_ascii_lowercase()
                .starts_with("multipart/form-data")
        })
        .unwrap_or(false);

    if !is_multipart {
        let filename = fallback_filename
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("upload.bin")
            .to_string();
        let content_type = content_type_header
            .and_then(|ct| ct.split(';').next())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        return Ok(ParsedUpload {
            filename,
            content_type,
            data: body.to_vec(),
        });
    }

    let boundary = content_type_header
        .and_then(extract_boundary)
        .ok_or_else(|| FileUploadError::InvalidMultipart("missing boundary parameter".into()))?;

    let delimiter = format!("--{boundary}").into_bytes();
    let mut cursor = find_subslice(body, &delimiter)
        .ok_or_else(|| FileUploadError::InvalidMultipart("multipart boundary not found".into()))?;

    let mut fallback_file_part: Option<ParsedUpload> = None;

    loop {
        cursor += delimiter.len();
        if cursor >= body.len() {
            break;
        }
        // Closing boundary "--"
        if body[cursor..].starts_with(b"--") {
            break;
        }
        if body[cursor..].starts_with(b"\r\n") {
            cursor += 2;
        } else if body[cursor..].starts_with(b"\n") {
            cursor += 1;
        }

        let remaining = &body[cursor..];
        let next_delim_rel = find_subslice(remaining, &delimiter).ok_or_else(|| {
            FileUploadError::InvalidMultipart("unterminated multipart section".into())
        })?;
        let part_slice = &remaining[..next_delim_rel];
        cursor += next_delim_rel;

        let (header_bytes, raw_data) = if let Some(pos) = find_subslice(part_slice, b"\r\n\r\n") {
            (&part_slice[..pos], &part_slice[pos + 4..])
        } else if let Some(pos) = find_subslice(part_slice, b"\n\n") {
            (&part_slice[..pos], &part_slice[pos + 2..])
        } else {
            continue;
        };

        let mut data = raw_data;
        if let Some(stripped) = data.strip_suffix(b"\r\n") {
            data = stripped;
        } else if let Some(stripped) = data.strip_suffix(b"\n") {
            data = stripped;
        }

        let headers_str = String::from_utf8_lossy(header_bytes);
        let mut disposition_filename: Option<String> = None;
        let mut disposition_name: Option<String> = None;
        let mut part_content_type: Option<String> = None;

        for line in headers_str.lines() {
            if let Some((name, val)) = line.split_once(':') {
                if name.trim().eq_ignore_ascii_case("content-disposition") {
                    disposition_filename = parse_disposition_param(val, "filename");
                    disposition_name = parse_disposition_param(val, "name");
                } else if name.trim().eq_ignore_ascii_case("content-type") {
                    let ct = val.trim();
                    if !ct.is_empty() {
                        part_content_type = Some(ct.to_string());
                    }
                }
            }
        }

        if let Some(fname) = disposition_filename.filter(|s| !s.trim().is_empty()) {
            if data.is_empty() {
                return Err(FileUploadError::EmptyFile);
            }
            return Ok(ParsedUpload {
                filename: fname,
                content_type: part_content_type,
                data: data.to_vec(),
            });
        }

        if disposition_name.as_deref() == Some("file") && !data.is_empty() {
            let fname = fallback_filename
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("upload.bin")
                .to_string();
            fallback_file_part = Some(ParsedUpload {
                filename: fname,
                content_type: part_content_type,
                data: data.to_vec(),
            });
        }
    }

    fallback_file_part.ok_or_else(|| {
        FileUploadError::InvalidMultipart("no file part found in multipart/form-data".into())
    })
}

pub async fn save_file_to_disk(
    config: &FileUploadConfig,
    original_name: &str,
    content_type: Option<&str>,
    data: &[u8],
    uploaded_by: Option<String>,
) -> Result<FileMetadata, FileUploadError> {
    if data.is_empty() {
        return Err(FileUploadError::EmptyFile);
    }
    if data.len() > config.max_file_size_bytes {
        return Err(FileUploadError::FileTooLarge {
            size_bytes: data.len(),
            max_bytes: config.max_file_size_bytes,
        });
    }

    let safe_original = sanitize_filename(original_name);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = FILE_SEQ.fetch_add(1, Ordering::Relaxed);
    let stored_name = format!("{nanos}-{seq}-{safe_original}");

    if !is_safe_stored_filename(&stored_name) {
        return Err(FileUploadError::InvalidFilename(stored_name));
    }

    tokio::fs::create_dir_all(&config.upload_dir).await?;
    let target_path = config.upload_dir.join(&stored_name);
    tokio::fs::write(&target_path, data).await?;

    let resolved_content_type = content_type
        .map(str::trim)
        .filter(|ct| !ct.is_empty() && !ct.eq_ignore_ascii_case("application/octet-stream"))
        .map(String::from)
        .unwrap_or_else(|| detect_content_type_from_ext(&stored_name).to_string());

    let base = config.public_base_path.trim_end_matches('/');
    let url = format!("{base}/{stored_name}");

    Ok(FileMetadata {
        original_name: safe_original,
        stored_name,
        content_type: resolved_content_type,
        size_bytes: data.len(),
        url,
        disk_path: target_path.to_string_lossy().into_owned(),
        uploaded_at: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        uploaded_by,
    })
}

pub async fn read_file_from_disk(
    config: &FileUploadConfig,
    stored_name: &str,
) -> Result<(Vec<u8>, &'static str), FileUploadError> {
    if !is_safe_stored_filename(stored_name) {
        return Err(FileUploadError::InvalidFilename(stored_name.to_string()));
    }

    let target_path = config.upload_dir.join(stored_name);
    let bytes = tokio::fs::read(&target_path).await.map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            FileUploadError::NotFound(stored_name.to_string())
        } else {
            FileUploadError::Io(err)
        }
    })?;

    Ok((bytes, detect_content_type_from_ext(stored_name)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_filename_strips_paths_and_unsafe_chars() {
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
        assert_eq!(
            sanitize_filename(r"C:\Users\Test\my receipt (1).png"),
            "my_receipt__1_.png"
        );
        assert_eq!(sanitize_filename("..."), "file.bin");
        assert_eq!(sanitize_filename("   "), "file.bin");
    }

    #[test]
    fn test_is_safe_stored_filename() {
        assert!(is_safe_stored_filename("12345-1-invoice.pdf"));
        assert!(!is_safe_stored_filename(""));
        assert!(!is_safe_stored_filename(".env"));
        assert!(!is_safe_stored_filename("../secret.txt"));
        assert!(!is_safe_stored_filename("sub/file.txt"));
        assert!(!is_safe_stored_filename(r"sub\file.txt"));
        assert!(!is_safe_stored_filename("a..b.txt"));
        assert!(!is_safe_stored_filename("bad\0.txt"));
    }

    #[test]
    fn test_parse_multipart_upload_binary_payload() {
        let boundary = "----WebKitFormBoundary7MA4YWxkTrZu0gW";
        let mut body = Vec::new();
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"note\"\r\n\r\nhello\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"photo.png\"\r\nContent-Type: image/png\r\n\r\n"
            )
            .as_bytes(),
        );
        let png_bytes = [0x89u8, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0xFF];
        body.extend_from_slice(&png_bytes);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

        let parsed = parse_upload_payload(
            Some(&format!("multipart/form-data; boundary=\"{boundary}\"")),
            None,
            &body,
        )
        .expect("multipart parse should succeed");

        assert_eq!(parsed.filename, "photo.png");
        assert_eq!(parsed.content_type.as_deref(), Some("image/png"));
        assert_eq!(parsed.data, png_bytes);
    }

    #[test]
    fn test_parse_raw_upload_payload() {
        let data = b"hello raw upload";
        let parsed = parse_upload_payload(Some("text/plain"), Some("note.txt"), data)
            .expect("raw parse should succeed");
        assert_eq!(parsed.filename, "note.txt");
        assert_eq!(parsed.content_type.as_deref(), Some("text/plain"));
        assert_eq!(parsed.data, data);
    }

    #[tokio::test]
    async fn test_save_and_read_file_disk_roundtrip() {
        let temp_dir = std::env::temp_dir().join(format!(
            "narayani_upload_test_{}_{}",
            std::process::id(),
            FILE_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let config = FileUploadConfig::new(&temp_dir);
        let payload = b"\x89PNG\r\n\x1a\nfake-png-content";

        let meta = save_file_to_disk(
            &config,
            "receipt.png",
            Some("image/png"),
            payload,
            Some("admin@narayanisangh.com".to_string()),
        )
        .await
        .expect("save should succeed");

        assert_eq!(meta.original_name, "receipt.png");
        assert_eq!(meta.content_type, "image/png");
        assert_eq!(meta.size_bytes, payload.len());
        assert_eq!(meta.url, format!("/files/{}", meta.stored_name));

        let resource = meta.to_file_resource();
        assert_eq!(resource.resource_name, Some(ResourceName::FileResource));
        assert!(resource.inner_content.is_some());

        let (read_bytes, detected_ct) = read_file_from_disk(&config, &meta.stored_name)
            .await
            .expect("read should succeed");
        assert_eq!(read_bytes, payload);
        assert_eq!(detected_ct, "image/png");

        let traversal = read_file_from_disk(&config, "../secret.txt").await;
        assert!(matches!(traversal, Err(FileUploadError::InvalidFilename(_))));

        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }
}
