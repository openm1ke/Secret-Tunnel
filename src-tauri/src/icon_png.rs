//! Application icons as PNG data URLs: a small encoder for icons read from
//! the system (Windows hands them over as raw pixels) and base64.
#![cfg_attr(not(windows), allow(dead_code))]

/// An uncompressed PNG of RGBA pixels, row by row. Icons are tiny, so
/// stored deflate blocks keep this simple at a few kilobytes each.
pub fn encode_rgba(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let row = width as usize * 4;
    let mut raw = Vec::with_capacity((row + 1) * height as usize);
    for line in rgba.chunks(row).take(height as usize) {
        raw.push(0); // filter: none
        raw.extend_from_slice(line);
    }
    let mut zlib = vec![0x78, 0x01];
    let mut blocks = raw.chunks(65_535).peekable();
    if blocks.peek().is_none() {
        zlib.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
    }
    while let Some(block) = blocks.next() {
        zlib.push(u8::from(blocks.peek().is_none()));
        let length = block.len() as u16;
        zlib.extend_from_slice(&length.to_le_bytes());
        zlib.extend_from_slice(&(!length).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA
    chunk(&mut png, b"IHDR", &header);
    chunk(&mut png, b"IDAT", &zlib);
    chunk(&mut png, b"IEND", &[]);
    png
}

fn chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    png.extend_from_slice(&(data.len() as u32).to_be_bytes());
    png.extend_from_slice(kind);
    png.extend_from_slice(data);
    let mut crc = 0xffff_ffff_u32;
    for &byte in kind.iter().chain(data) {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    png.extend_from_slice(&(!crc).to_be_bytes());
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1_u32, 0_u32);
    for &byte in data {
        a = (a + u32::from(byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

pub fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for (index, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            out.push(if index <= chunk.len() { TABLE[(n >> shift) as usize & 63] as char } else { '=' });
        }
    }
    out
}

pub fn data_url(png: &[u8]) -> String {
    format!("data:image/png;base64,{}", base64(png))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard_alphabet() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn encoded_icons_are_valid_pngs() {
        // 48x48 is what Windows hands over; 200x200 spans several deflate blocks.
        for size in [48_u32, 200] {
            let pixels: Vec<u8> = (0..size * size).flat_map(|i| [(i % 251) as u8, 120, (i / size) as u8, 200]).collect();
            let png = encode_rgba(size, size, &pixels);
            assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
            assert_eq!(&png[12..16], b"IHDR");
            assert!(data_url(&png).starts_with("data:image/png;base64,iVBORw0KGgo"));
            #[cfg(target_os = "macos")]
            {
                // macOS can read it back, which checks the CRCs and the zlib stream.
                let path = std::env::temp_dir().join(format!("secret-tunnel-icon-{size}-{}.png", std::process::id()));
                std::fs::write(&path, &png).unwrap();
                let out = std::process::Command::new("/usr/bin/sips").args(["-g", "pixelWidth"]).arg(&path).output().unwrap();
                let _ = std::fs::remove_file(&path);
                assert!(String::from_utf8_lossy(&out.stdout).contains(&format!("pixelWidth: {size}")), "{}", String::from_utf8_lossy(&out.stdout));
            }
        }
    }
}
