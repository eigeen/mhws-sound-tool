mod bnk;
mod cmd;
mod config;
mod ffmpeg;
mod pck;
mod utils;
mod wwise;

use std::{
    env,
    fs::{self, File},
    io::{self, Read, Seek, Write},
    path::{Path, PathBuf},
    sync::LazyLock,
};

use bnk::SectionPayload;
use colored::Colorize;
use config::{BinConfig, Config};
use dialoguer::{Input, theme::ColorfulTheme};
use eyre::{Context, eyre};
use ffmpeg::FFmpegCli;
use indexmap::IndexMap;
use regex::Regex;
use serde::{Deserialize, Serialize};
use wwise::{WwiseConsole, WwiseSource};

// [001]12345678 .wem
static REG_WEM_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[(\d{3,4})\](\d+)").unwrap());

#[derive(Debug, Clone, Serialize, Deserialize)]
enum SoundToolProject {
    Bnk(BnkProject),
    Pck(PckProject),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BnkProject {
    metadata_file: String,
    source_file_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PckProject {
    metadata_file: String,
    source_file_name: String,
}

fn dump_bank(bank: &bnk::Bnk, output_path: impl AsRef<Path>) -> eyre::Result<()> {
    let mut didx_entries = vec![];

    for section in &bank.sections {
        match &section.payload {
            SectionPayload::Didx { entries } => {
                didx_entries = entries.clone();
            }
            SectionPayload::Data { data_list } => {
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
                        let file_path = output_path.as_ref().join(file_name);
                        println!("{}: {}", "Wem".green().dimmed(), file_path.display());
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
            SectionPayload::Didx { .. } | SectionPayload::Data { .. }
        )
    });
    let meta_bank_path = output_path.as_ref().join("bank.json");
    println!(
        "{}: {}",
        "Metadata".green().dimmed(),
        meta_bank_path.display()
    );
    let mut meta_bank_file = File::create(&meta_bank_path)
        .context("Failed to create bank meta file")
        .context(format!("Path: {}", meta_bank_path.display()))?;
    let mut writer = io::BufWriter::new(&mut meta_bank_file);
    serde_json::to_writer(&mut writer, &meta_bank).context("Failed to write bank meta to file")?;

    Ok(())
}

fn dump_pck<R>(mut reader: R, output_path: impl AsRef<Path>) -> eyre::Result<()>
where
    R: Read + Seek,
{
    let pck = pck::PckHeader::from_reader(&mut reader)?;
    for i in 0..pck.wem_entries.len() {
        let entry = &pck.wem_entries[i];
        let file_name = if pck.wem_entries.len() < 1000 {
            format!("[{:03}]{}.wem", i, entry.id)
        } else {
            format!("[{:04}]{}.wem", i, entry.id)
        };
        let file_path = output_path.as_ref().join(file_name);
        println!("{}: {}", "Wem".green().dimmed(), file_path.display());
        let mut file = File::create(&file_path)
            .context("Failed to create wem output file")
            .context(format!("Path: {}", file_path.display()))?;

        let mut wem_reader = pck.wem_reader(&mut reader, i).unwrap();
        io::copy(&mut wem_reader, &mut file).context("Failed to write wem data to file")?;
    }

    // 导出其余部分
    let meta_pck_path = output_path.as_ref().join("pck.json");
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

    println!("{}: {}", "Export".cyan(), output_path.as_ref().display());

    Ok(())
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

fn package_bank(
    project: &BnkProject,
    project_path: impl AsRef<Path>,
    output_root: impl AsRef<Path>,
) -> eyre::Result<()> {
    let project_path = project_path.as_ref();
    let output_root = output_root.as_ref();

    let bank_meta_path = project_path.join(&project.metadata_file);
    if !bank_meta_path.is_file() {
        eyre::bail!("Bnk metadata file not found: {}", bank_meta_path.display())
    }
    let bank_meta_content = fs::read_to_string(&bank_meta_path)?;
    let mut bank: bnk::Bnk = serde_json::from_str(&bank_meta_content)?;

    // 导出bnk
    // 读取wem
    let mut wem_files = vec![];
    for entry in fs::read_dir(project_path)? {
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
        bnk::Section::new(SectionPayload::Didx {
            entries: didx_entries,
        }),
    );
    bank.sections.insert(
        2,
        bnk::Section::new(SectionPayload::Data {
            data_list: wem_files.into_iter().map(|wem| wem.data).collect(),
        }),
    );

    // 导出bank
    // project dir name
    let mut output_path = output_root
        .join(&project.source_file_name)
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

    println!("{}: {}", "Export".cyan(), output_path);

    Ok(())
}

fn package_pck(
    project: &PckProject,
    project_path: impl AsRef<Path>,
    output_root: impl AsRef<Path>,
) -> eyre::Result<()> {
    let project_path = project_path.as_ref();
    let output_root = output_root.as_ref();

    let pck_header_path = project_path.join(&project.metadata_file);
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
    for entry in fs::read_dir(project_path)? {
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
        .join(&project.source_file_name)
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

    println!("{}: {}", "Export".cyan(), output_path);

    Ok(())
}

fn handle_project(
    project_path: impl AsRef<Path>,
    output_root: impl AsRef<Path>,
) -> eyre::Result<()> {
    let project_path = project_path.as_ref();

    let project_json_path = project_path.join("project.json");
    if !project_json_path.is_file() {
        eyre::bail!(
            "Project metadata file not found: {}",
            project_json_path.display()
        )
    }
    let project_content = fs::read_to_string(project_json_path)?;
    let project: SoundToolProject =
        serde_json::from_str(&project_content).context("Failed to parse project data")?;
    match project {
        SoundToolProject::Bnk(bnk_project) => {
            package_bank(&bnk_project, project_path, output_root)
                .context("Failed to package bank")?;
        }
        SoundToolProject::Pck(pck_project) => {
            package_pck(&pck_project, project_path, output_root)
                .context("Failed to package PCK")?;
        }
    }

    Ok(())
}

fn create_project_metadata(
    dir_path: impl AsRef<Path>,
    data: &SoundToolProject,
) -> eyre::Result<()> {
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
    serde_json::to_writer(&mut writer, &data).context("Failed to write project data to file")?;
    Ok(())
}

/// Get ffmpeg instance, from config, or update config with user input.
fn request_ffmpeg() -> eyre::Result<FFmpegCli> {
    let mut config = Config::global().lock();
    if let Some(ffmpeg_config) = config.get_bin_config("ffmpeg") {
        return FFmpegCli::new_with_path(PathBuf::from(&ffmpeg_config.path))
            .ok_or(eyre::eyre!("FFmpeg not found"));
    }

    println!(
        "{}: ffmpeg path is not set, please setup in config.toml.",
        "Warning".yellow().bold()
    );
    let ffmpeg_path: String = Input::with_theme(&ColorfulTheme::default())
        .show_default(true)
        .default("ffmpeg.exe".to_string())
        .with_prompt("Input ffmpeg path")
        .interact_text()
        .unwrap();
    let ffmpeg_path = ffmpeg_path.trim_matches(['\"', '\'']);
    let ffmpeg =
        FFmpegCli::new_with_path(PathBuf::from(ffmpeg_path)).ok_or(eyre!("FFmpeg not found"))?;
    config.set_bin_config("ffmpeg", ffmpeg.program_path().to_string_lossy().as_ref());
    config.save();

    Ok(ffmpeg)
}

fn request_wwise_console() -> eyre::Result<WwiseConsole> {
    let mut config = Config::global().lock();
    if let Some(wconsole_config) = config.get_bin_config("WwiseConsole") {
        return Ok(WwiseConsole::new_with_path(PathBuf::from(
            &wconsole_config.path,
        ))?);
    }

    println!(
        "{}: WwiseConsole path is not set, please setup in config.toml.",
        "Warning".yellow().bold()
    );
    let wconsole_path: String = Input::with_theme(&ColorfulTheme::default())
        .with_prompt("Input WwiseConsole.exe path")
        .interact_text()
        .unwrap();
    let wconsole_path = wconsole_path.trim_matches(['\"', '\'']);
    let wconsole = WwiseConsole::new_with_path(PathBuf::from(wconsole_path))?;
    config.set_bin_config(
        "WwiseConsole",
        wconsole.program_path().to_string_lossy().as_ref(),
    );
    config.save();

    Ok(wconsole)
}

fn single_file_to_wem(input: impl AsRef<Path>) -> eyre::Result<()> {
    let input = input.as_ref().canonicalize().context(format!(
        "Failed to canonicalize input path: {}",
        input.as_ref().display()
    ))?;

    let wconsole = request_wwise_console()?;
    let project = wconsole.acquire_temp_project()?;

    let input_dir = input.parent().unwrap();
    let input_file = input.file_name().unwrap();
    let mut source = WwiseSource::new(input_dir.to_str().unwrap());
    source.add_source(input_file.to_str().unwrap());
    project.convert_external_source(&source, input_dir.to_str().unwrap())?;

    // mv to root
    let output_dir = input_dir.join("Windows");
    for entry in output_dir.read_dir()? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            fs::copy(
                &path,
                input_dir.parent().unwrap().join(path.file_name().unwrap()),
            )?;
        }
    }

    Ok(())
}

fn main_entry() -> eyre::Result<()> {
    let args = env::args().collect::<Vec<_>>();
    if args.len() < 2 {
        eyre::bail!("Usage: {} <input> ...", args[0]);
    }

    // config init
    {
        let mut config = Config::global().lock();
        if config.get_bin_config("ffmpeg").is_none() {
            if let Ok(ffmpeg) = FFmpegCli::new() {
                config.set_bin_config("ffmpeg", ffmpeg.program_path().to_string_lossy().as_ref());
            }
        }
        if config.get_bin_config("WwiseConsole").is_none() {
            if let Ok(wwise_console) = WwiseConsole::new() {
                config.set_bin_config(
                    "WwiseConsole",
                    wwise_console.program_path().to_string_lossy().as_ref(),
                );
            }
        }
    }

    let input_paths = args[1..].iter().map(Path::new).collect::<Vec<_>>();

    for path in input_paths {
        if !path.exists() {
            eyre::bail!("File or directory not found: {}", path.display())
        }
        if path.is_dir() {
            let output_root = path.parent().unwrap_or(Path::new("."));
            handle_project(path, output_root)?;
        } else {
            let file = File::open(path)?;
            let mut reader = io::BufReader::new(file);
            let mut magic = [0; 4];
            reader.read_exact(&mut magic)?;
            reader.seek(io::SeekFrom::Start(0))?;
            if &magic == b"BKHD" {
                // bnk file
                println!("{}: {}", "Wwise Sound Bank".green(), path.display());
                let bank = bnk::Bnk::from_reader(&mut reader)
                    .map_err(|e| eyre::Report::new(e))
                    .context("Failed to parse bank")?;
                let mut bank_dump_output = path.to_string_lossy().to_string();
                bank_dump_output.push_str(".project");
                fs::create_dir_all(&bank_dump_output)?;
                dump_bank(&bank, &bank_dump_output).context("Failed to dump bank")?;
                // 创建project.json
                let project_data = SoundToolProject::Bnk(BnkProject {
                    metadata_file: "bank.json".to_string(),
                    source_file_name: path.file_name().unwrap().to_string_lossy().to_string(),
                });
                create_project_metadata(&bank_dump_output, &project_data)?;
            } else if &magic == b"AKPK" {
                // pck file
                println!("{}: {}", "Wwise PCK".green(), path.display());
                let mut output_path = path.to_string_lossy().to_string();
                output_path.push_str(".project");
                fs::create_dir_all(&output_path)?;
                dump_pck(reader, &output_path).context("Failed to dump PCK")?;
                // 创建project.json
                let project_data = SoundToolProject::Pck(PckProject {
                    metadata_file: "pck.json".to_string(),
                    source_file_name: path.file_name().unwrap().to_string_lossy().to_string(),
                });
                create_project_metadata(&output_path, &project_data)?;
            } else {
                // magic not match, other file type
                let file_ext = path.extension().unwrap_or_default().to_string_lossy();
                match file_ext.as_ref() {
                    "mp3" | "ogg" | "aac" => {
                        // transcode to wav, and to wem
                        let ffmpeg = request_ffmpeg()?;
                        let ff_out_dir = Path::new("sound-tool-temp");
                        if !ff_out_dir.exists() {
                            fs::create_dir_all(ff_out_dir)?;
                        }
                        let ff_out_file = ff_out_dir.join(path.file_name().unwrap());
                        ffmpeg.simple_transcode(path, &ff_out_file)?;
                        // to wem
                        single_file_to_wem(&ff_out_file)?;
                    }
                    "wav" => {
                        // to wem
                        single_file_to_wem(path)?;
                    }
                    _ => {
                        eyre::bail!("Unsupported input file type: {}", path.display());
                    }
                }
            }
        }
    }

    Ok(())
}

fn main() -> eyre::Result<()> {
    println!(
        "{} v{}{}",
        "MHWS Sound Tool".magenta().bold(),
        env!("CARGO_PKG_VERSION"),
        " - by @Eigeen".dimmed()
    );

    if let Err(e) = main_entry() {
        println!("{}{:#}", "Error: ".red(), e);
    }
    wait_for_exit();

    Ok(())
}

fn wait_for_exit() {
    let _: String = Input::new()
        .allow_empty(true)
        .with_prompt("Press Enter to exit")
        .interact()
        .unwrap();
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
}
