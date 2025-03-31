use std::io;

use byteorder::{LE, ReadBytesExt, WriteBytesExt};

use serde::{Deserialize, Serialize};

type Result<T> = std::result::Result<T, BnkError>;

#[derive(Debug, thiserror::Error)]
pub enum BnkError {
    #[error("IO error: {0}")]
    IO(#[from] std::io::Error),

    #[error("Accessing DATA section before DIDX section.")]
    MissingDidx,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bnk {
    pub sections: Vec<Section>,
}

impl Bnk {
    pub fn from_reader<R>(reader: &mut R) -> Result<Self>
    where
        R: io::Read + io::Seek,
    {
        let mut sections = Vec::new();
        loop {
            let mut magic = [0u8; 4];
            if let Err(e) = reader.read_exact(&mut magic) {
                if e.kind() == io::ErrorKind::UnexpectedEof {
                    break;
                }
            };
            let section = if &magic == b"DATA" {
                let total_length = reader.read_u32::<LE>()?;
                let didx_entries = sections
                    .iter()
                    .find_map(|sec: &Section| {
                        if let SectionPayload::Didx { entries } = &sec.payload {
                            Some(entries)
                        } else {
                            None
                        }
                    })
                    .ok_or(BnkError::MissingDidx)?;
                let data_start_pos = reader.stream_position()?;
                let mut data_list = Vec::with_capacity(didx_entries.len());
                for entry in didx_entries {
                    let mut data = vec![0; entry.length as usize];
                    reader.seek(io::SeekFrom::Start(data_start_pos + entry.offset as u64))?;
                    reader.read_exact(&mut data)?;
                    data_list.push(data);
                }
                reader.seek(io::SeekFrom::Start(data_start_pos + total_length as u64))?;
                Section {
                    magic,
                    section_length: total_length,
                    payload: SectionPayload::Data { data_list },
                }
            } else {
                Section::from_reader(reader, magic)?
            };
            sections.push(section);
        }
        Ok(Bnk { sections })
    }

    pub fn write_to<W>(&self, writer: &mut W) -> Result<()>
    where
        W: io::Write + io::Seek,
    {
        let mut didx_entries: Option<&[DidxEntry]> = None;

        for section in &self.sections {
            writer.write_all(&section.magic)?;
            writer.write_u32::<LE>(section.section_length)?;

            match &section.payload {
                SectionPayload::Bkhd {
                    version,
                    id,
                    unknown,
                } => {
                    writer.write_u32::<LE>(*version)?;
                    writer.write_u32::<LE>(*id)?;
                    writer.write_all(unknown)?;
                }
                SectionPayload::Didx { entries } => {
                    didx_entries.replace(entries);
                    for entry in entries {
                        let entry_bytes: [u8; 12] = unsafe { std::mem::transmute(entry.clone()) };
                        writer.write_all(&entry_bytes)?;
                    }
                }
                SectionPayload::Hirc { entries } => {
                    writer.write_u32::<LE>(entries.len() as u32)?;
                    for entry in entries {
                        entry.write_to(writer)?;
                    }
                }
                SectionPayload::Data { data_list } => {
                    let Some(didx_entries) = didx_entries else {
                        return Err(BnkError::MissingDidx);
                    };
                    let data_start_pos = writer.stream_position()?;
                    for (i, data) in data_list.iter().enumerate() {
                        let entry = &didx_entries[i];
                        writer.seek(io::SeekFrom::Start(data_start_pos + entry.offset as u64))?;
                        writer.write_all(data)?;
                        // 16字节对齐 padding
                    }
                    // 移动到padding末尾
                    writer.seek(io::SeekFrom::Start(
                        data_start_pos + section.section_length as u64,
                    ))?;
                }
                SectionPayload::Unk { data } => {
                    writer.write_all(data)?;
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Section {
    pub magic: [u8; 4],
    pub section_length: u32,
    #[serde(flatten)]
    pub payload: SectionPayload,
}

impl Section {
    pub fn new(payload: SectionPayload) -> Self {
        match &payload {
            SectionPayload::Didx { entries } => Section {
                magic: *b"DIDX",
                section_length: entries.len() as u32 * size_of::<DidxEntry>() as u32,
                payload,
            },
            SectionPayload::Data { data_list } => {
                let mut total_length = 0;
                for data in data_list {
                    total_length += data.len();
                }
                Section {
                    magic: *b"DATA",
                    section_length: total_length as u32,
                    payload,
                }
            }
            _ => unimplemented!("Section::new for payload: {:#?}", payload),
        }
    }

    fn from_reader<R>(reader: &mut R, magic: [u8; 4]) -> Result<Self>
    where
        R: io::Read + io::Seek,
    {
        let section_length = reader.read_u32::<LE>()?;
        let payload = match &magic {
            b"BKHD" => SectionPayload::Bkhd {
                version: reader.read_u32::<LE>()?,
                id: reader.read_u32::<LE>()?,
                unknown: {
                    let mut unknown = vec![0; section_length as usize - 8];
                    reader.read_exact(&mut unknown)?;
                    unknown
                },
            },
            b"DIDX" => {
                let entry_count = (section_length as usize) / size_of::<DidxEntry>();
                let mut entries = Vec::with_capacity(entry_count);
                for _ in 0..entry_count {
                    let mut buf = [0; size_of::<DidxEntry>()];
                    reader.read_exact(&mut buf)?;
                    entries.push(unsafe { std::mem::transmute::<[u8; 12], DidxEntry>(buf) });
                }
                SectionPayload::Didx { entries }
            }
            b"HIRC" => {
                let count = reader.read_u32::<LE>()?;
                let mut entries = Vec::with_capacity(count as usize);
                for _ in 0..count {
                    let entry_type = reader.read_u8()?;
                    // let entry_type = HircEntryType::from_repr(entry_type).ok_or(
                    //     Error::UnknownHircEntryType(reader.stream_position()?, entry_type),
                    // )?;
                    entries.push(HircEntry::from_reader(reader, entry_type)?);
                }
                SectionPayload::Hirc { entries }
            }
            b"DATA" => {
                unreachable!("DATA section should be handled separately.");
            }
            _ => {
                let mut data = vec![0; section_length as usize];
                reader.read_exact(&mut data)?;
                SectionPayload::Unk { data }
            }
        };

        Ok(Section {
            magic,
            section_length,
            payload,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "content")]
pub enum SectionPayload {
    Bkhd {
        version: u32,
        id: u32,
        unknown: Vec<u8>,
    },
    Didx {
        entries: Vec<DidxEntry>,
    },
    Hirc {
        entries: Vec<HircEntry>,
    },
    Data {
        data_list: Vec<Vec<u8>>,
    },
    Unk {
        data: Vec<u8>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HircEntry {
    pub type_id: u8,
    pub length: u32,
    pub id: u32,
    pub data: Vec<u8>,
}

impl HircEntry {
    fn from_reader<R>(reader: &mut R, type_id: u8) -> Result<Self>
    where
        R: io::Read + io::Seek,
    {
        let length = reader.read_u32::<LE>()?;
        let id = reader.read_u32::<LE>()?;
        let mut data = vec![0; length as usize - 4];
        reader.read_exact(&mut data)?;
        Ok(HircEntry {
            type_id,
            length,
            id,
            data,
        })
    }

    fn write_to<W>(&self, writer: &mut W) -> Result<()>
    where
        W: io::Write,
    {
        writer.write_u8(self.type_id)?;
        writer.write_u32::<LE>(self.length)?;
        writer.write_u32::<LE>(self.id)?;
        writer.write_all(&self.data)?;
        Ok(())
    }
}

#[repr(C)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DidxEntry {
    pub id: u32,
    pub offset: u32,
    pub length: u32,
}

#[cfg(test)]
mod tests {
    use std::{fs, io};

    use super::*;

    const INPUT_HIRC: &str = "test_files/Wp00_Cmn.sbnk.1.X64";
    const INPUT_HIRC_2: &str = "test_files/Wp00_Cmn_Effect.sbnk.1.X64";
    const INPUT_DIDX_DATA: &str = "test_files/Wp00_Cmn_m.sbnk.1.X64";

    #[test]
    fn test_hirc() {
        let input = fs::read(INPUT_HIRC).unwrap();
        let mut reader = io::Cursor::new(input);
        let sbnk = Bnk::from_reader(&mut reader).unwrap();
        assert_eq!(&sbnk.sections[0].magic, b"BKHD");
        eprintln!("{:?}", sbnk.sections[0]);
    }

    #[test]
    fn test_hirc_2() {
        let input = fs::read(INPUT_HIRC_2).unwrap();
        let mut reader = io::Cursor::new(input);
        let _sbnk = Bnk::from_reader(&mut reader).unwrap();
    }

    #[test]
    fn test_didx_data() {
        let input = fs::read(INPUT_DIDX_DATA).unwrap();
        let mut reader = io::Cursor::new(input);
        let _sbnk = Bnk::from_reader(&mut reader).unwrap();
        eprintln!("didx: {:?}", _sbnk.sections[1])
    }
}
