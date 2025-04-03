use std::{
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::LazyLock,
};

use colored::Colorize;
use eyre::Context;
use indexmap::IndexMap;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::{bnk, pck};

// [001]12345678
static REG_WEM_NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\[(\d+)\](\d+)").unwrap());

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SoundToolProject {
    Bnk(BnkProject),
    Pck(PckProject),
}

impl SoundToolProject {
    pub fn from_path(path: impl AsRef<Path>) -> eyre::Result<Self> {
        let project_path = path.as_ref();

        let project_json_path = project_path.join("project.json");
        if !project_json_path.is_file() {
            eyre::bail!(
                "Project metadata file not found: {}",
                project_json_path.display()
            )
        }
        let project_content = fs::read_to_string(project_json_path)
            .context("Failed to read project metadata file")?;
        let mut project: SoundToolProject =
            serde_json::from_str(&project_content).context("Failed to parse project data")?;
        project.set_project_path(project_path);

        Ok(project)
    }

    pub fn repack(&self, output_root: impl AsRef<Path>) -> eyre::Result<()> {
        match self {
            SoundToolProject::Bnk(project) => project.repack(output_root),
            SoundToolProject::Pck(project) => project.repack(output_root),
        }
    }

    pub fn dump_bnk(
        input_path: impl AsRef<Path>,
        output_root: impl AsRef<Path>,
    ) -> eyre::Result<Self> {
        let input_path = input_path.as_ref();
        let output_root = output_root.as_ref();

        let file = File::open(input_path)?;
        let mut reader = io::BufReader::new(file);
        let bank = bnk::Bnk::from_reader(&mut reader)
            .map_err(|e| eyre::Report::new(e))
            .context("Failed to parse bnk file")?;
        let source_name = input_path.file_name().unwrap().to_string_lossy();
        let mut project_path = output_root
            .join(source_name.as_ref())
            .to_string_lossy()
            .to_string();
        project_path.push_str(".project");
        let project_path = PathBuf::from(project_path);
        fs::create_dir_all(&project_path).context("Failed to create project directory")?;

        // dump bnk data
        let mut didx_entries = vec![];

        for section in &bank.sections {
            match &section.payload {
                bnk::SectionPayload::Didx { entries } => {
                    didx_entries = entries.clone();
                }
                bnk::SectionPayload::Data { data_list } => {
                    if didx_entries.is_empty() {
                        eyre::bail!("DIDX section must before DATA section.")
                    }
                    data_list
                        .iter()
                        .enumerate()
                        .zip(didx_entries.iter())
                        .try_for_each(|((idx, data), entry)| -> eyre::Result<()> {
                            let file_name = if didx_entries.len() < 1000 {
                                format!("[{:03}]{}.wem", idx, entry.id)
                            } else {
                                format!("[{:04}]{}.wem", idx, entry.id)
                            };
                            let file_path = project_path.join(file_name);
                            let mut file = File::create(&file_path)
                                .context("Failed to create wem output file")
                                .context(format!("Path: {}", file_path.display()))?;
                            file.write_all(data)
                                .context("Failed to write wem data to file")?;
                            Ok(())
                        })?;
                }
                _ => {}
            }
        }

        // 导出其余部分
        let mut meta_bank = bank.clone();
        meta_bank.sections.retain(|sec| {
            !matches!(
                &sec.payload,
                bnk::SectionPayload::Didx { .. } | bnk::SectionPayload::Data { .. }
            )
        });
        let meta_bank_path = project_path.join("bank.json");
        println!(
            "{}: {}",
            "Metadata".green().dimmed(),
            meta_bank_path.display()
        );
        let mut meta_bank_file = File::create(&meta_bank_path)
            .context("Failed to create bank meta file")
            .context(format!("Path: {}", meta_bank_path.display()))?;
        let mut writer = io::BufWriter::new(&mut meta_bank_file);
        serde_json::to_writer(&mut writer, &meta_bank)
            .context("Failed to write bank meta to file")?;

        // 创建project
        let this = Self::Bnk(BnkProject {
            metadata_file: "bank.json".to_string(),
            source_file_name: source_name.to_string(),
            project_path: PathBuf::from(&project_path),
        });
        this.write_project_metadata(project_path)
            .context("Failed to write project metadata")?;

        Ok(this)
    }

    pub fn dump_pck(
        input_path: impl AsRef<Path>,
        output_root: impl AsRef<Path>,
    ) -> eyre::Result<Self> {
        let input_path = input_path.as_ref();
        let output_root = output_root.as_ref();

        let file = File::open(input_path)?;
        let mut reader = io::BufReader::new(file);
        let pck = pck::PckHeader::from_reader(&mut reader)
            .map_err(|e| eyre::Report::new(e))
            .context("Failed to parse pck file")?;
        let source_name = input_path.file_name().unwrap().to_string_lossy();
        let mut project_path = output_root
            .join(source_name.as_ref())
            .to_string_lossy()
            .to_string();
        project_path.push_str(".project");
        let project_path = PathBuf::from(&project_path);
        fs::create_dir_all(&project_path).context("Failed to create project directory")?;

        // dump pck data
        for i in 0..pck.wem_entries.len() {
            let entry = &pck.wem_entries[i];
            let file_name = if pck.wem_entries.len() < 1000 {
                format!("[{:03}]{}.wem", i, entry.id)
            } else {
                format!("[{:04}]{}.wem", i, entry.id)
            };
            let file_path = project_path.join(file_name);
            let mut file = File::create(&file_path)
                .context("Failed to create wem output file")
                .context(format!("Path: {}", file_path.display()))?;

            let mut wem_reader = pck.wem_reader(&mut reader, i).unwrap();
            io::copy(&mut wem_reader, &mut file).context("Failed to write wem data to file")?;
        }

        // 导出其余部分
        let meta_pck_path = project_path.join("pck.json");
        println!(
            "{}: {}",
            "Metadata".green().dimmed(),
            meta_pck_path.display()
        );
        let mut meta_pck_file = File::create(&meta_pck_path)
            .context("Failed to create pck meta file")
            .context(format!("Path: {}", meta_pck_path.display()))?;
        let mut writer = io::BufWriter::new(&mut meta_pck_file);
        serde_json::to_writer(&mut writer, &pck).context("Failed to write pck meta to file")?;

        // 创建project
        let this = Self::Pck(PckProject {
            metadata_file: "pck.json".to_string(),
            source_file_name: source_name.to_string(),
            project_path: project_path.clone(),
        });
        this.write_project_metadata(project_path)
            .context("Failed to write project metadata")?;

        Ok(this)
    }

    fn set_project_path(&mut self, project_path: impl AsRef<Path>) {
        match self {
            SoundToolProject::Bnk(project) => {
                project.project_path = project_path.as_ref().to_path_buf()
            }
            SoundToolProject::Pck(project) => {
                project.project_path = project_path.as_ref().to_path_buf()
            }
        }
    }

    /// Create project metadata file `project.json`.
    fn write_project_metadata(&self, dir_path: impl AsRef<Path>) -> eyre::Result<()> {
        let metadata_path = dir_path.as_ref().join("project.json");
        println!(
            "{}: {}",
            "Project".green().dimmed(),
            metadata_path.display()
        );
        let mut project_file = File::create(&metadata_path)
            .context("Failed to create project file")
            .context(format!("Path: {}", metadata_path.display()))?;
        let mut writer = io::BufWriter::new(&mut project_file);
        serde_json::to_writer(&mut writer, &self)
            .context("Failed to write project data to file")?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BnkProject {
    metadata_file: String,
    source_file_name: String,
    #[serde(skip)]
    project_path: PathBuf,
}

impl BnkProject {
    pub fn repack(&self, output_root: impl AsRef<Path>) -> eyre::Result<()> {
        let output_root = output_root.as_ref();

        let bank_meta_path = self.project_path.join(&self.metadata_file);
        if !bank_meta_path.is_file() {
            eyre::bail!("Bnk metadata file not found: {}", bank_meta_path.display())
        }
        let bank_meta_content = fs::read_to_string(&bank_meta_path)?;
        let mut bank: bnk::Bnk = serde_json::from_str(&bank_meta_content)?;

        // 导出bnk
        // 读取wem
        let mut wem_files = vec![];
        for entry in fs::read_dir(&self.project_path)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() || path.extension().unwrap_or_default() != "wem" {
                continue;
            }

            // 解析wem文件名
            struct WemInfo {
                idx: u32,
                id: u32,
                data: Vec<u8>,
            }
            let file_stem = path.file_stem().unwrap().to_string_lossy();
            let (idx, id) = parse_wem_name(&file_stem)?;
            let data = fs::read(path)?;
            wem_files.push(WemInfo { idx, id, data });
        }

        wem_files.sort_by_key(|wem| wem.idx);
        // 构造didx
        let mut didx_entries = vec![];
        let mut offset = 0;
        for wem in &wem_files {
            didx_entries.push(bnk::DidxEntry {
                id: wem.id,
                offset,
                length: wem.data.len() as u32,
            });
            // no padding
            offset += wem.data.len() as u32;
        }

        // 构造bank
        bank.sections.insert(
            1,
            bnk::Section::new(bnk::SectionPayload::Didx {
                entries: didx_entries,
            }),
        );
        bank.sections.insert(
            2,
            bnk::Section::new(bnk::SectionPayload::Data {
                data_list: wem_files.into_iter().map(|wem| wem.data).collect(),
            }),
        );

        // 导出bank
        // project dir name
        let mut output_path = output_root
            .join(&self.source_file_name)
            .to_string_lossy()
            .to_string();
        loop {
            if Path::new(&output_path).exists() {
                output_path.push_str(".new");
            } else {
                break;
            }
        }

        let output_file = File::create(&output_path)?;
        let mut writer = io::BufWriter::new(output_file);
        bank.write_to(&mut writer)?;

        println!("{}: {}", "Output".cyan(), output_path);

        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PckProject {
    metadata_file: String,
    source_file_name: String,
    #[serde(skip)]
    project_path: PathBuf,
}

impl PckProject {
    pub fn repack(&self, output_root: impl AsRef<Path>) -> eyre::Result<()> {
        let output_root = output_root.as_ref();

        let pck_header_path = self.project_path.join(&self.metadata_file);
        if !pck_header_path.is_file() {
            eyre::bail!("PCK metadata file not found: {}", pck_header_path.display())
        }
        let pck_header_content = fs::read_to_string(&pck_header_path)?;
        let mut pck_header: pck::PckHeader = serde_json::from_str(&pck_header_content)?;

        // 读取wem信息
        struct WemMetadata {
            idx: u32,
            file_size: u32,
            file_path: String,
        }
        let mut wem_metadata_map = IndexMap::new();
        for entry in fs::read_dir(&self.project_path)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() || path.extension().unwrap_or_default() != "wem" {
                continue;
            }

            // 解析wem文件名
            let file_stem = path.file_stem().unwrap().to_string_lossy();
            let (idx, id) = parse_wem_name(&file_stem)?;
            wem_metadata_map.insert(
                id,
                WemMetadata {
                    idx,
                    file_size: path.metadata()?.len() as u32,
                    file_path: path.to_string_lossy().to_string(),
                },
            );
        }
        wem_metadata_map.sort_unstable_by(|_, value_a, _, value_b| value_a.idx.cmp(&value_b.idx));
        // 更新header中的原始wem entries
        // 移除无效wem entries
        let mut drop_idx_list = vec![];
        for (i, entry) in pck_header.wem_entries.iter().enumerate() {
            if !wem_metadata_map.contains_key(&entry.id) {
                drop_idx_list.push(i);
            }
        }
        for i in drop_idx_list.iter().rev() {
            let entry = pck_header.wem_entries.remove(*i);
            println!(
                "{}: Wem file {} included in original PCK, but not found in project, removed.",
                "Warning".yellow(),
                entry.id
            );
        }
        if !drop_idx_list.is_empty() {
            println!(
                "{}: Wem count changed, will affect the original order ID, please use Wem unique ID as reference.",
                "Warning".yellow()
            );
        }
        // 更新数据
        let mut offset = pck_header.get_wem_offset_start();
        for entry in pck_header.wem_entries.iter_mut() {
            let metadata = wem_metadata_map.get(&entry.id).unwrap();
            entry.offset = offset;
            entry.length = metadata.file_size;
            offset += metadata.file_size;
        }

        let mut output_path = output_root
            .join(&self.source_file_name)
            .to_string_lossy()
            .to_string();
        loop {
            if Path::new(&output_path).exists() {
                output_path.push_str(".new");
            } else {
                break;
            }
        }
        // 导出pck header
        let output_file = File::create(&output_path)?;
        let mut writer = io::BufWriter::new(output_file);
        pck_header.write_to(&mut writer)?;
        // 写入wem
        for metadata in wem_metadata_map.values() {
            let file_path = Path::new(&metadata.file_path);
            let mut input_file = File::open(file_path)?;
            io::copy(&mut input_file, &mut writer)?;
        }

        println!("{}: {}", "Output".cyan(), output_path);

        Ok(())
    }
}

/// 解析Wem名，返回 (index, id)
fn parse_wem_name(name: &str) -> eyre::Result<(u32, u32)> {
    let name = name.trim();
    if let Some(captures) = REG_WEM_NAME.captures(name) {
        let idx = captures.get(1).and_then(|m| m.as_str().parse::<u32>().ok());
        let id = captures.get(2).and_then(|m| m.as_str().parse::<u32>().ok());
        let Some(id) = id else {
            eyre::bail!("Bad Wem file name, cannot parse Wem id. {}", name)
        };
        Ok((idx.unwrap_or(u32::MAX), id))
    } else {
        eyre::bail!("Bad Wem file name. {}", name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wem_name_regex() {
        let cases = [
            ("[001]12345678.wem", (1, 12345678)),
            ("[012]98765432.wem", (12, 98765432)),
            ("[999]99999999.wem", (999, 99999999)),
            ("[000]00000000.wem", (0, 0)),
        ];
        for (name, expected) in cases {
            let captures = REG_WEM_NAME.captures(name).unwrap();
            let idx = captures.get(1).unwrap().as_str().parse::<u32>().unwrap();
            let id = captures.get(2).unwrap().as_str().parse::<u32>().unwrap();
            assert_eq!(idx, expected.0);
            assert_eq!(id, expected.1);
        }
    }

    #[test]
    fn test_dump_bnk() {
        SoundToolProject::dump_bnk("test_files/Wp00_Cmn_m.sbnk.1.X64", "test_files").unwrap();
        assert!(Path::new("test_files/Wp00_Cmn_m.sbnk.1.X64.project/project.json").is_file());
        assert!(Path::new("test_files/Wp00_Cmn_m.sbnk.1.X64.project/bank.json").is_file());
        fs::remove_dir_all("test_files/Wp00_Cmn_m.sbnk.1.X64.project").unwrap();
    }

    #[test]
    fn test_dump_pck() {
        SoundToolProject::dump_pck("test_files/Cat_cmn_m.spck.1.X64", "test_files").unwrap();
        assert!(Path::new("test_files/Cat_cmn_m.spck.1.X64.project/project.json").is_file());
        assert!(Path::new("test_files/Cat_cmn_m.spck.1.X64.project/pck.json").is_file());
        fs::remove_dir_all("test_files/Cat_cmn_m.spck.1.X64.project").unwrap();
    }

    #[test]
    fn test_repack_bnk() {
        let path = "test_files/Wp00_Cmn_m.sbnk.1.X64".to_string();
        SoundToolProject::dump_bnk(&path, "test_files").unwrap();
        let mut project_path = path.clone();
        project_path.push_str(".project");
        let project = SoundToolProject::from_path(&project_path).unwrap();
        project.repack("test_files").unwrap();
        assert!(Path::new("test_files/Wp00_Cmn_m.sbnk.1.X64.new").is_file());
        fs::remove_file("test_files/Wp00_Cmn_m.sbnk.1.X64.new").unwrap();
        fs::remove_dir_all(&project_path).unwrap();
    }

    #[test]
    fn test_repack_pck() {
        let path = "test_files/Cat_cmn_m.spck.1.X64".to_string();
        SoundToolProject::dump_pck(&path, "test_files").unwrap();
        let mut project_path = path.clone();
        project_path.push_str(".project");
        let project = SoundToolProject::from_path(&project_path).unwrap();
        project.repack("test_files").unwrap();
        assert!(Path::new("test_files/Cat_cmn_m.spck.1.X64.new").is_file());
        fs::remove_file("test_files/Cat_cmn_m.spck.1.X64.new").unwrap();
        fs::remove_dir_all(&project_path).unwrap();
    }
}
