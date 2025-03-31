use std::io;

use byteorder::{LE, ReadBytesExt};

/// Create String from UTF-16 string bytes with null terminator.
pub fn string_from_utf16_reader<R: io::Read>(reader: &mut R) -> io::Result<String> {
    let mut utf16_buf = vec![];
    loop {
        let char = reader.read_u16::<LE>()?;
        if char == 0 {
            break;
        }
        utf16_buf.push(char);
    }
    String::from_utf16(&utf16_buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

pub fn string_to_utf16_bytes(s: &str) -> Vec<u8> {
    s.encode_utf16()
        .chain(Some(0))
        .flat_map(|wc| wc.to_le_bytes())
        .collect()
}

/// Calculate the size of data written by a function that writes to a writer.
pub fn calc_write_size<F, W>(writer: &mut W, f: F) -> io::Result<u64>
where
    F: FnOnce(&mut W) -> io::Result<()>,
    W: io::Write + io::Seek,
{
    let pos = writer.stream_position()?;
    f(writer)?;
    Ok(writer.stream_position()? - pos)
}
