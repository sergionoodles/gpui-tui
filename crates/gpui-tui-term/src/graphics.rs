//! Encoder for kitty graphics protocol commands (`ESC _ G … ESC \`).
//!
//! Spec: <https://sw.kovidgoyal.net/kitty/graphics-protocol/>

use std::fmt::Write as _;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;

const COMMAND_START: &[u8] = b"\x1b_G";
const COMMAND_END: &[u8] = b"\x1b\\";
/// Maximum base64 payload per escape sequence for direct transmission.
const CHUNK_SIZE: usize = 4096;
/// Base64 of a single black RGB pixel, used by queries.
const ONE_PIXEL_RGB_BASE64: &str = "AAAA";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    Rgb,
    Rgba,
}

impl PixelFormat {
    fn code(self) -> u8 {
        match self {
            Self::Rgb => 24,
            Self::Rgba => 32,
        }
    }

    pub fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgb => 3,
            Self::Rgba => 4,
        }
    }
}

/// How the pixel data reaches the terminal.
#[derive(Clone, Copy, Debug)]
pub enum Medium<'a> {
    /// Pixels inside the escape sequence, base64 encoded and optionally zlib-compressed.
    Direct { data: &'a [u8], compressed: bool },
    /// Name of a POSIX shared memory object; the terminal reads and unlinks it.
    SharedMemory { name: &'a str },
}

/// Where and how large the image is drawn, in cells, starting at the cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellArea {
    pub columns: u16,
    pub rows: u16,
}

#[derive(Clone, Copy, Debug)]
pub struct Image {
    pub image_id: u32,
    pub placement_id: u32,
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
}

/// Transmits an image and places it at the cursor, scaled to `area`. Re-using the same image
/// and placement IDs replaces the previous frame atomically. Responses are suppressed and the
/// cursor does not move.
pub fn transmit_and_place(out: &mut Vec<u8>, image: &Image, area: CellArea, medium: Medium<'_>) {
    let mut keys = format!(
        "a=T,q=2,C=1,f={},s={},v={},i={},p={},c={},r={}",
        image.format.code(),
        image.width,
        image.height,
        image.image_id,
        image.placement_id,
        area.columns,
        area.rows,
    );
    write_medium(out, &mut keys, medium);
}

/// Asks the terminal whether it can load a one-pixel image through `medium`. The reply is a
/// graphics response carrying `image_id`; nothing is stored or displayed.
pub fn query(out: &mut Vec<u8>, image_id: u32, medium: QueryMedium<'_>) {
    let mut keys = format!("a=q,i={image_id},s=1,v=1,f=24");
    match medium {
        QueryMedium::Direct => {
            keys.push_str(",t=d");
            push_command(out, &keys, ONE_PIXEL_RGB_BASE64.as_bytes());
        }
        QueryMedium::SharedMemory { name } => {
            keys.push_str(",t=s");
            push_command(out, &keys, BASE64.encode(name).as_bytes());
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum QueryMedium<'a> {
    Direct,
    SharedMemory { name: &'a str },
}

/// Deletes every image and placement and frees their storage.
pub fn delete_all(out: &mut Vec<u8>) {
    push_command(out, "a=d,d=A,q=2", b"");
}

fn write_medium(out: &mut Vec<u8>, keys: &mut String, medium: Medium<'_>) {
    match medium {
        Medium::SharedMemory { name } => {
            keys.push_str(",t=s");
            push_command(out, keys, BASE64.encode(name).as_bytes());
        }
        Medium::Direct { data, compressed } => {
            keys.push_str(",t=d");
            if compressed {
                keys.push_str(",o=z");
            }
            push_chunked(out, keys, &BASE64.encode(data));
        }
    }
}

/// Splits a direct-transmission payload into chunks; only the first carries the keys.
fn push_chunked(out: &mut Vec<u8>, keys: &str, payload: &str) {
    let chunks: Vec<&[u8]> = payload.as_bytes().chunks(CHUNK_SIZE).collect();
    let last_index = chunks.len().saturating_sub(1);
    if chunks.is_empty() {
        push_command(out, keys, b"");
        return;
    }
    for (index, chunk) in chunks.into_iter().enumerate() {
        let more = u8::from(index != last_index);
        let mut chunk_keys = String::new();
        if index == 0 {
            chunk_keys.push_str(keys);
            chunk_keys.push(',');
        }
        // Writing to a String cannot fail.
        let _ = write!(chunk_keys, "m={more}");
        push_command(out, &chunk_keys, chunk);
    }
}

fn push_command(out: &mut Vec<u8>, keys: &str, payload: &[u8]) {
    out.extend_from_slice(COMMAND_START);
    out.extend_from_slice(keys.as_bytes());
    if !payload.is_empty() {
        out.push(b';');
        out.extend_from_slice(payload);
    }
    out.extend_from_slice(COMMAND_END);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image() -> Image {
        Image {
            image_id: 1,
            placement_id: 1,
            width: 2,
            height: 1,
            format: PixelFormat::Rgba,
        }
    }

    fn area() -> CellArea {
        CellArea {
            columns: 80,
            rows: 24,
        }
    }

    #[test]
    fn shared_memory_frame_is_one_command() {
        let mut out = Vec::new();
        transmit_and_place(
            &mut out,
            &image(),
            area(),
            Medium::SharedMemory { name: "/frame" },
        );
        assert_eq!(
            String::from_utf8_lossy(&out),
            "\x1b_Ga=T,q=2,C=1,f=32,s=2,v=1,i=1,p=1,c=80,r=24,t=s;L2ZyYW1l\x1b\\"
        );
    }

    #[test]
    fn direct_payload_is_chunked_with_keys_only_on_first_chunk() {
        let data = vec![7u8; 6000];
        let mut out = Vec::new();
        transmit_and_place(
            &mut out,
            &image(),
            area(),
            Medium::Direct {
                data: &data,
                compressed: true,
            },
        );
        let text = String::from_utf8_lossy(&out);
        let commands: Vec<&str> = text
            .split("\x1b\\")
            .filter(|command| !command.is_empty())
            .collect();
        assert_eq!(commands.len(), 2);
        assert!(commands[0].starts_with("\x1b_Ga=T,"));
        assert!(commands[0].contains(",t=d,o=z,m=1;"));
        assert!(commands[1].starts_with("\x1b_Gm=0;"));

        let payload: String = commands
            .iter()
            .filter_map(|command| command.split_once(';').map(|(_, payload)| payload))
            .collect();
        assert_eq!(BASE64.decode(payload).ok(), Some(data));
    }

    #[test]
    fn queries_and_delete() {
        let mut out = Vec::new();
        query(&mut out, 31, QueryMedium::Direct);
        delete_all(&mut out);
        assert_eq!(
            String::from_utf8_lossy(&out),
            "\x1b_Ga=q,i=31,s=1,v=1,f=24,t=d;AAAA\x1b\\\x1b_Ga=d,d=A,q=2\x1b\\"
        );
    }
}
