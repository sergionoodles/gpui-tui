//! Decoding of kitty graphics commands, the way a terminal would.

use std::collections::HashMap;
use std::fs::File;
use std::io::Read as _;

use anyhow::{Context as _, Result, anyhow, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use flate2::read::ZlibDecoder;
use image::RgbaImage;
use rustix::fs::Mode;
use rustix::shm;

/// A parsed `ESC _ G keys ; payload ESC \` command.
#[derive(Debug)]
pub(crate) struct Command {
    keys: HashMap<String, String>,
    payload: Vec<u8>,
}

impl Command {
    pub(crate) fn parse(apc: &[u8]) -> Option<Self> {
        let body = apc.strip_prefix(b"G")?;
        let (keys, payload) = match body.iter().position(|&byte| byte == b';') {
            Some(index) => (&body[..index], body[index + 1..].to_vec()),
            None => (body, Vec::new()),
        };
        let keys = std::str::from_utf8(keys)
            .ok()?
            .split(',')
            .filter_map(|pair| pair.split_once('='))
            .map(|(key, value)| (key.to_owned(), value.to_owned()))
            .collect();
        Some(Self { keys, payload })
    }

    pub(crate) fn key(&self, key: &str) -> Option<&str> {
        self.keys.get(key).map(String::as_str)
    }

    fn number(&self, key: &str) -> Option<u32> {
        self.key(key).and_then(|value| value.parse().ok())
    }

    pub(crate) fn action(&self) -> &str {
        self.key("a").unwrap_or("t")
    }

    pub(crate) fn image_id(&self) -> u32 {
        self.number("i").unwrap_or(0)
    }

    pub(crate) fn has_more_chunks(&self) -> bool {
        self.key("m") == Some("1")
    }
}

/// Collects chunked direct transmissions until the last chunk arrives.
#[derive(Default)]
pub(crate) struct Assembler {
    pending: Option<(Command, Vec<u8>)>,
}

impl Assembler {
    /// Returns the complete command (first chunk's keys, all chunks' payload) when done.
    pub(crate) fn push(&mut self, command: Command) -> Option<Command> {
        match self.pending.take() {
            Some((first, mut payload)) => {
                payload.extend_from_slice(&command.payload);
                if command.has_more_chunks() {
                    self.pending = Some((first, payload));
                    return None;
                }
                Some(Command { payload, ..first })
            }
            None if command.has_more_chunks() => {
                let payload = command.payload.clone();
                self.pending = Some((command, payload));
                None
            }
            None => Some(command),
        }
    }
}

/// A transmitted image and the cell area it was placed over.
pub(crate) struct DecodedImage {
    pub(crate) image: RgbaImage,
    pub(crate) columns: u32,
    pub(crate) rows: u32,
}

/// Loads the pixels of a transmit command. Shared memory segments are unlinked after reading,
/// as a real terminal does.
pub(crate) fn decode_image(command: &Command) -> Result<DecodedImage> {
    let width = command.number("s").context("missing width")?;
    let height = command.number("v").context("missing height")?;
    let format = command.number("f").unwrap_or(32);
    let raw = load_payload(command)?;
    let raw = if command.key("o") == Some("z") {
        let mut inflated = Vec::new();
        ZlibDecoder::new(raw.as_slice()).read_to_end(&mut inflated)?;
        inflated
    } else {
        raw
    };
    let rgba = match format {
        32 => raw,
        24 => raw
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|&[red, green, blue]| [red, green, blue, u8::MAX])
            .collect(),
        other => bail!("unsupported pixel format f={other}"),
    };
    let image = RgbaImage::from_raw(width, height, rgba)
        .ok_or_else(|| anyhow!("pixel data does not match {width}x{height}"))?;
    Ok(DecodedImage {
        image,
        columns: command.number("c").unwrap_or(0),
        rows: command.number("r").unwrap_or(0),
    })
}

pub(crate) fn load_payload(command: &Command) -> Result<Vec<u8>> {
    let decoded = BASE64.decode(&command.payload)?;
    match command.key("t").unwrap_or("d") {
        "d" => Ok(decoded),
        "s" => {
            let name = String::from_utf8(decoded)?;
            read_shared_memory(&name)
        }
        other => bail!("unsupported transmission medium t={other}"),
    }
}

fn read_shared_memory(name: &str) -> Result<Vec<u8>> {
    let fd = shm::open(name, shm::OFlags::RDONLY, Mode::empty())
        .with_context(|| format!("shared memory segment {name} not found"))?;
    let mut data = Vec::new();
    File::from(fd).read_to_end(&mut data)?;
    shm::unlink(name)?;
    Ok(data)
}
