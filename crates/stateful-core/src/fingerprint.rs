use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{self, Read},
    path::Path,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentFingerprint {
    pub exists: bool,
    pub byte_len: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

impl ContentFingerprint {
    pub const fn missing() -> Self {
        Self {
            exists: false,
            byte_len: 0,
            sha256: None,
        }
    }

    pub fn is_complete_exact(&self) -> bool {
        match self {
            Self {
                exists: false,
                byte_len: 0,
                sha256: None,
            } => true,
            Self {
                exists: true,
                sha256: Some(hash),
                ..
            } => hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()),
            _ => false,
        }
    }
}

pub fn fingerprint_path(path: &Path) -> io::Result<ContentFingerprint> {
    match File::open(path) {
        Ok(file) => fingerprint_reader(file),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(ContentFingerprint::missing()),
        Err(error) => Err(error),
    }
}

pub fn fingerprint_reader(mut reader: impl Read) -> io::Result<ContentFingerprint> {
    const BUFFER_SIZE: usize = 64 * 1024;
    let mut hasher = Sha256::new();
    let mut byte_len = 0_u64;
    let mut buffer = [0_u8; BUFFER_SIZE];

    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        byte_len += read as u64;
    }

    Ok(ContentFingerprint {
        exists: true,
        byte_len,
        sha256: Some(format!("{:x}", hasher.finalize())),
    })
}
