use std::io;

use byteorder::{LE, ReadBytesExt, WriteBytesExt};
use serde::{Deserialize, Serialize};

use crate::utils;

type Result<T> = std::result::Result<T, PckError>;

#[derive(Debug, thiserror::Error)]
pub enum PckError {
    #[error("IO error: {0}")]
    IO(#[from] std::io::Error),

    #[error("Invalid magic of PCK file: {0:X?}")]
    InvalidMagic([u8; 4]),
    #[error("Assertion failed: {0}")]
    Assertion(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PckHeader {
    pub header_length: u32,
    pub version: u32,
    pub string_table: Vec<PckString>,
    pub bnk_entries: Vec<PckFileEntry>,
    pub wem_entries: Vec<PckFileEntry>,
    pub external_entries: Vec<u32>,
    #[serde(skip)]
    bnk_positions: Vec<u32>,
    #[serde(skip)]
    wem_positions: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileType {
    Bnk,
    Wem,
}

impl PckHeader {
    pub fn from_reader<R>(reader: &mut R) -> Result<Self>
    where
        R: io::Read + io::Seek,
    {
        let mut magic = [0; 4];
        reader.read_exact(&mut magic)?;
        if &magic != b"AKPK" {
            return Err(PckError::InvalidMagic(magic));
        }
        let header_length = reader.read_u32::<LE>()?;
        let version = reader.read_u32::<LE>()?;
        let language_length = reader.read_u32::<LE>()?;
        let bnk_table_length = reader.read_u32::<LE>()?;
        let wem_table_length = reader.read_u32::<LE>()?;
        let external_table_length = reader.read_u32::<LE>()?;

        // read strings
        #[derive(Debug)]
        struct PckStringEntry {
            offset: u32,
            index: u32,
        }
        let string_start_pos = reader.stream_position()?;
        let string_count = reader.read_u32::<LE>()?;
        let mut entries = Vec::with_capacity(string_count as usize);
        for _ in 0..string_count {
            entries.push(PckStringEntry {
                offset: reader.read_u32::<LE>()?,
                index: reader.read_u32::<LE>()?,
            });
        }
        let mut string_table = Vec::with_capacity(string_count as usize);
        for entry in entries {
            reader.seek(io::SeekFrom::Start(string_start_pos + entry.offset as u64))?;
            let wstr = utils::string_from_utf16_reader(reader)?;
            string_table.push(PckString {
                index: entry.index,
                value: wstr,
            });
        }
        reader.seek(io::SeekFrom::Start(
            string_start_pos + language_length as u64,
        ))?;

        let bnk_count = reader.read_u32::<LE>()?;
        let mut bnk_entries = Vec::with_capacity(bnk_count as usize);
        for _ in 0..bnk_count {
            let mut buf = [0u8; 20];
            reader.read_exact(&mut buf)?;
            let entry: PckFileEntry = unsafe { std::mem::transmute(buf) };
            bnk_entries.push(entry);
        }

        let wem_count = reader.read_u32::<LE>()?;
        let mut wem_entries = Vec::with_capacity(wem_count as usize);
        for _ in 0..wem_count {
            let mut buf = [0u8; 20];
            reader.read_exact(&mut buf)?;
            let entry: PckFileEntry = unsafe { std::mem::transmute(buf) };
            wem_entries.push(entry);
        }

        let mut unk_struct_data = vec![0u32; external_table_length as usize / 4];
        for i in 0..(external_table_length / 4) {
            unk_struct_data[i as usize] = reader.read_u32::<LE>()?;
        }

        let mut header = PckHeader {
            header_length,
            version,
            string_table,
            bnk_entries,
            wem_entries,
            external_entries: unk_struct_data,
            bnk_positions: Vec::new(),
            wem_positions: Vec::new(),
        };

        header.calculate_file_positions();

        Ok(header)
    }

    fn calculate_file_positions(&mut self) {
        let mut all_entries: Vec<(PckFileEntry, FileType)> = self
            .bnk_entries
            .iter()
            .map(|e| (e.clone(), FileType::Bnk))
            .chain(self.wem_entries.iter().map(|e| (e.clone(), FileType::Wem)))
            .collect();

        all_entries.sort_by_key(|(entry, _)| entry.offset);
        
        let mut sorted_positions = Vec::with_capacity(all_entries.len());
        let mut current_pos = self.get_data_offset_start();

        for (entry, _) in &all_entries {
            let alignment = entry.padding_block_size as u32;

            if alignment > 1 && current_pos % alignment != 0 {
                current_pos += alignment - (current_pos % alignment);
            }
            
            sorted_positions.push(current_pos);
            current_pos += entry.length as u32;
        }
        
        let mut pos_map = std::collections::HashMap::new();
        for (i, (entry, _)) in all_entries.iter().enumerate() {
            pos_map.insert(entry.id, sorted_positions[i]);
        }

        self.bnk_positions = self.bnk_entries
            .iter()
            .map(|e| *pos_map.get(&e.id).unwrap_or(&0))
            .collect();
            
        self.wem_positions = self.wem_entries
            .iter()
            .map(|e| *pos_map.get(&e.id).unwrap_or(&0))
            .collect();
    }

    pub fn get_data_offset_start(&self) -> u32 {
        self.header_size() as u32 + 8 // 4 (magic) + 4 (header_length)
    }

    pub fn wem_reader<'a, R>(&'a self, reader: R, index: usize) -> Option<PckFileReader<'a, R>>
    where
        R: io::Read + io::Seek,
    {
        if index >= self.wem_entries.len() {
            return None;
        }
        let entry = &self.wem_entries[index];
        let start_pos = self.wem_positions[index];
        
        Some(PckFileReader::new(reader, entry, u64::from(start_pos)))
    }

    pub fn bnk_reader<'a, R>(&'a self, reader: R, index: usize) -> Option<PckFileReader<'a, R>>
    where
        R: io::Read + io::Seek,
    {
        if index >= self.bnk_entries.len() {
            return None;
        }
        let entry = &self.bnk_entries[index];
        let start_pos = self.bnk_positions[index];
        
        Some(PckFileReader::new(reader, entry, u64::from(start_pos)))
    }

    pub fn write_to<W>(&self, writer: &mut W) -> io::Result<()>
    where
        W: io::Write + io::Seek,
    {
        writer.write_all(b"AKPK")?;
        writer.write_u32::<LE>(0)?; // header_length
        writer.write_u32::<LE>(self.version)?;
        writer.write_u32::<LE>(0)?; // language_length
        writer.write_u32::<LE>(0)?; // bnk_table_length
        writer.write_u32::<LE>(0)?; // wem_table_length
        writer.write_u32::<LE>(0)?; // external_table_length

        // write strings
        let language_size = utils::calc_write_size(writer, |writer| {
            writer.write_u32::<LE>(self.string_table.len() as u32)?; // string_count
            let mut utf16_strings = vec![];
            for string in &self.string_table {
                utf16_strings.push(utils::string_to_utf16_bytes(&string.value));
            }
            // calculate offsets and write string entries
            let mut offset = size_of::<u32>() + size_of::<u32>() * 2 * self.string_table.len();
            utf16_strings.iter().zip(&self.string_table).try_for_each(
                |(utf16_bytes, pck_string)| -> io::Result<()> {
                    writer.write_u32::<LE>(offset as u32)?;
                    writer.write_u32::<LE>(pck_string.index)?;
                    offset += utf16_bytes.len();
                    Ok(())
                },
            )?;
            // write string data
            for utf16_bytes in utf16_strings {
                writer.write_all(&utf16_bytes)?;
            }
            Ok(())
        })?;

        writer.write_u32::<LE>(self.bnk_entries.len() as u32)?;
        for entry in &self.bnk_entries {
            let buf: [u8; 20] = unsafe { std::mem::transmute(entry.clone()) };
            writer.write_all(&buf)?;
        }

        writer.write_u32::<LE>(self.wem_entries.len() as u32)?;
        for entry in &self.wem_entries {
            let buf: [u8; 20] = unsafe { std::mem::transmute(entry.clone()) };
            writer.write_all(&buf)?;
        }
        for data in &self.external_entries {
            writer.write_u32::<LE>(*data)?;
        }

        let bnk_table_size = self.bnk_table_size();
        let wem_table_size = self.wem_table_size();
        let unk_struct_size = self.external_entries_size();
        let header_size = size_of::<u32>() * 5
            + language_size as usize
            + bnk_table_size
            + wem_table_size
            + unk_struct_size;
        let end_pos = writer.stream_position()?;

        writer.seek(io::SeekFrom::Start(4))?;
        writer.write_u32::<LE>(header_size as u32)?;
        writer.seek(io::SeekFrom::Current(4))?;
        writer.write_u32::<LE>(language_size as u32)?;
        writer.write_u32::<LE>(bnk_table_size as u32)?;
        writer.write_u32::<LE>(wem_table_size as u32)?;
        writer.write_u32::<LE>(unk_struct_size as u32)?;

        writer.seek(io::SeekFrom::Start(end_pos))?;

        Ok(())
    }

    fn header_size(&self) -> usize {
        self.bnk_table_size()
            + self.wem_table_size()
            + self.external_entries_size()
            + self.language_size()
            + size_of::<u32>() * 5 // unk + size(val)*4
    }

    fn bnk_table_size(&self) -> usize {
        4 + self.bnk_entries.len() * size_of::<PckFileEntry>()
    }

    fn wem_table_size(&self) -> usize {
        // entries_count(val) + entries_size
        4 + self.wem_entries.len() * size_of::<PckFileEntry>()
    }

    fn external_entries_size(&self) -> usize {
        self.external_entries.len() * 4
    }

    fn language_size(&self) -> usize {
        let mut size = 0;
        // strings size
        for string in &self.string_table {
            size += utils::string_to_utf16_bytes(&string.value).len();
        }
        // entries size = count(val) + entry*count
        size += 4 + self.string_table.len() * 8;
        size
    }
}

#[repr(C)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PckFileEntry {
    pub id: u32,
    pub padding_block_size: u32,
    pub length: u32,
    pub offset: u32,
    pub language_id: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PckString {
    pub index: u32,
    pub value: String,
}

pub struct PckFileReader<'a, R> {
    reader: R,
    entry: &'a PckFileEntry,
    start_pos: u64,
    read_size: usize,
}

impl<'a, R> PckFileReader<'a, R>
where
    R: io::Read + io::Seek,
{
    fn new(reader: R, entry: &'a PckFileEntry, start_pos: u64) -> Self {
        PckFileReader {
            reader,
            entry,
            start_pos,
            read_size: 0,
        }
    }
}

impl<R> io::Read for PckFileReader<'_, R>
where
    R: io::Read + io::Seek,
{
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.read_size == 0 && self.entry.length > 0 {
            self.reader.seek(io::SeekFrom::Start(self.start_pos))?;
        }
        
        let available = self.entry.length as usize - self.read_size;
        if available == 0 {
            return Ok(0);
        }
        
        let read_limit = buf.len().min(available);
        if read_limit == 0 {
            return Ok(0); 
        }

        let bytes_read = self.reader.read(&mut buf[..read_limit])?;
        self.read_size += bytes_read;
        Ok(bytes_read)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        io::{Cursor, Read},
    };

    use super::*;

    const INPUT: &str = "test_files/Cat_cmn_m.spck.1.X64";

    #[test]
    fn test_pck_from_reader() {
        let mut input = fs::read(INPUT).unwrap();
        let mut reader = io::Cursor::new(&mut input);
        let pck = PckHeader::from_reader(&mut reader).unwrap();
        assert_eq!(pck.wem_entries.len(), 333);
        assert_eq!(pck.language_size(), 20);
        assert_eq!(pck.bnk_table_size(), 4);
        assert_eq!(pck.wem_table_size(), 6664);
        assert_eq!(pck.external_entries_size(), 4);
        assert_eq!(pck.header_size(), 6712);
        assert_eq!(pck.get_data_offset_start(), 6720);
        // eprintln!("pck: {:?}", pck);
        for i in 0..pck.wem_entries.len() {
            let mut wem_reader = pck.wem_reader(Cursor::new(&mut input), i).unwrap();
            let mut buf = vec![];
            wem_reader.read_to_end(&mut buf).unwrap();
            assert_eq!(buf.len(), pck.wem_entries[i].length as usize);
            assert_eq!(&buf[0..4], b"RIFF");
        }
    }
}
