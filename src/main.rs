mod bnk;
mod config;
mod ffmpeg;
mod pck;
mod project;
mod transcode;
mod utils;
mod wwise;

use std::{
    env, fs,
    io::Read,
    path::{Path, PathBuf},
    sync::atomic::{self, AtomicBool},
};

use clap::Parser;
use colored::Colorize;
use config::Config;
use dialoguer::Input;
use eyre::Context;
use log::{error, info};
use project::SoundToolProject;

static INTERACTIVE_MODE: AtomicBool = AtomicBool::new(true);

#[derive(Debug, Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
    /// No interactive mode, so that the program
    /// won't block waiting for user input.
    #[arg(long, default_value = "false")]
    no_interact: bool,
}

#[derive(Debug, clap::Subcommand)]
enum Command {
    PackageProject(CmdPackageProject),
    UnpackBundle(CmdUnpackBundle),
    SoundToWem(CmdSoundToWem),
}

#[derive(Debug, clap::Args)]
struct CmdPackageProject {
    /// Input project directory path.
    #[arg(short, long)]
    input: String,
    /// Output root path.
    #[arg(short, long)]
    output: Option<String>,
}

#[derive(Debug, clap::Args)]
struct CmdUnpackBundle {
    /// Input bundle file path.
    ///
    /// Support BNK and PCK formats.
    #[arg(short, long)]
    input: String,
    /// Output root path.
    #[arg(short, long)]
    output: Option<String>,
}

#[derive(Debug, clap::Args)]
struct CmdSoundToWem {
    /// Input sound file path.
    ///
    /// Support WAV, OGG, AAC, FLAC, MP3 formats.
    #[arg(short, long)]
    input: Vec<String>,
    /// Output directory path.
    ///
    /// The output file name will be the same as the input file name,
    /// with the extension changed to .wem
    #[arg(short, long)]
    output: Option<String>,
    /// WwiseConsole program path.
    #[arg(long)]
    wwise_console: String,
    /// FFmpeg program path.
    ///
    /// If input files contain non-wav format,
    /// this option is required.
    #[arg(long)]
    ffmpeg: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputFileType {
    Project,
    GeneralAudio(&'static str),
    Wem,
    Bnk,
    Pck,
}

impl InputFileType {
    pub fn from_path(path: impl AsRef<Path>) -> Option<Self> {
        let path = path.as_ref();
        if !path.exists() {
            return None;
        }
        if path.is_dir() {
            // check project.json
            if path.join("project.json").is_file() {
                return Some(InputFileType::Project);
            } else {
                return None;
            }
        }

        // ext check
        let file_ext = path.extension().and_then(|ext| ext.to_str())?;
        let result = match file_ext {
            "wav" => Some(InputFileType::GeneralAudio("wav")),
            "ogg" => Some(InputFileType::GeneralAudio("ogg")),
            "aac" => Some(InputFileType::GeneralAudio("aac")),
            "flac" => Some(InputFileType::GeneralAudio("flac")),
            "mp3" => Some(InputFileType::GeneralAudio("mp3")),
            _ => None,
        };
        if result.is_some() {
            return result;
        }

        // magic check
        let mut magic = [0; 4];
        let mut file = std::fs::File::open(path).ok()?;
        file.read_exact(&mut magic).ok()?;
        match &magic {
            b"BKHD" => Some(InputFileType::Bnk),
            b"AKPK" => Some(InputFileType::Pck),
            b"RIFF" => Some(InputFileType::Wem),
            _ => None,
        }
    }

    #[allow(clippy::match_like_matches_macro)]
    pub fn similar_to(&self, other: &Self) -> bool {
        match (self, other) {
            (InputFileType::GeneralAudio(_), InputFileType::GeneralAudio(_)) => true,
            (InputFileType::Wem, InputFileType::Wem) => true,
            (InputFileType::Bnk, InputFileType::Bnk) => true,
            (InputFileType::Pck, InputFileType::Pck) => true,
            _ => false,
        }
    }
}

fn main() -> eyre::Result<()> {
    std::panic::set_hook(Box::new(panic_hook));

    println!(
        "{} v{}{}",
        "MHWS Sound Tool".magenta().bold(),
        env!("CARGO_PKG_VERSION"),
        " - by @Eigeen".dimmed()
    );

    // init logger
    let mut builder = env_logger::builder();
    if cfg!(feature = "log_info") {
        builder.filter_level(log::LevelFilter::Info);
    } else {
        builder.filter_level(log::LevelFilter::Debug);
    }
    builder.format_timestamp(None).init();

    if let Err(e) = main_entry() {
        error!("{:#}", e);
    }
    wait_for_exit();

    Ok(())
}

fn panic_hook(info: &std::panic::PanicHookInfo) {
    println!("{}: {:#?}", "Panic".red().bold(), info);
    wait_for_exit();
    std::process::exit(1);
}

fn main_entry() -> eyre::Result<()> {
    // drag and drop support, try to detect if all params are file paths
    let args = env::args().collect::<Vec<_>>();
    if args.len() < 2 {
        eyre::bail!("Usage: {} <input> ...", args[0]);
    }

    let mut input_paths = vec![];
    for path in args.iter().skip(1) {
        let path = Path::new(path);
        if !path.exists() {
            break;
        }
        input_paths.push(path);
    }

    if input_paths.len() != args.len() - 1 {
        // not all params are file paths, use cli parser
        let cli = Cli::parse();
        return cli_main(&cli);
    }

    // direct input mode
    let file_types = input_paths
        .iter()
        .map(InputFileType::from_path)
        .collect::<Vec<_>>();
    // require all same known file type
    if file_types.iter().any(|t| t.is_none()) {
        eyre::bail!("Input paths contain unsupported file type");
    }
    let file_type = file_types[0].as_ref().unwrap();
    for t in file_types.iter().skip(1) {
        let t = t.as_ref().unwrap();
        if !t.similar_to(file_type) {
            eyre::bail!("Input paths must be of the same type");
        }
    }
    // build cli args
    match file_type {
        InputFileType::Project => {
            for input in input_paths {
                let cmd = Command::PackageProject(CmdPackageProject {
                    input: input.to_string_lossy().to_string(),
                    output: None,
                });
                let cli = Cli {
                    command: cmd,
                    no_interact: false,
                };
                cli_main(&cli)?;
            }
        }
        InputFileType::GeneralAudio(_) => {
            let cmd = Command::SoundToWem(CmdSoundToWem {
                input: input_paths
                    .iter()
                    .map(|p| p.to_string_lossy().to_string())
                    .collect(),
                output: None,
                ffmpeg: None,
                wwise_console: "".to_string(),
            });
            let cli = Cli {
                command: cmd,
                no_interact: false,
            };
            cli_main(&cli)?;
        }
        InputFileType::Bnk | InputFileType::Pck => {
            for input in input_paths {
                let cmd = Command::UnpackBundle(CmdUnpackBundle {
                    input: input.to_string_lossy().to_string(),
                    output: None,
                });
                let cli = Cli {
                    command: cmd,
                    no_interact: false,
                };
                cli_main(&cli)?;
            }
        }
        _ => {
            eyre::bail!("Unsupported input file type {:?}", file_type);
        }
    };

    Ok(())
}

fn cli_main(cli: &Cli) -> eyre::Result<()> {
    if cli.no_interact {
        INTERACTIVE_MODE.store(false, atomic::Ordering::SeqCst);
    }
    match &cli.command {
        Command::PackageProject(cmd) => {
            info!("Input: {}", cmd.input);
            if let Some(output) = &cmd.output {
                info!("Output: {}", output);
            }
            let project =
                SoundToolProject::from_path(&cmd.input).context("Failed to load project")?;

            let output_root = cmd.output.as_ref().map(PathBuf::from).unwrap_or_else(|| {
                Path::new(&cmd.input)
                    .parent()
                    .unwrap_or_else(|| {
                        let input_dir = Path::new(&cmd.input).parent().unwrap_or(Path::new("."));
                        input_dir
                    })
                    .to_path_buf()
            });
            project
                .repack(&output_root)
                .context("Failed to repack project")?;
        }
        Command::UnpackBundle(cmd) => {
            let input = Path::new(&cmd.input);
            if !input.is_file() {
                eyre::bail!("Input file not found: {}", input.display())
            }
            info!("Input: {}", cmd.input);
            if let Some(output) = &cmd.output {
                info!("Output: {}", output);
            }
            let output_root = cmd
                .output
                .as_ref()
                .map(PathBuf::from)
                .unwrap_or_else(|| input.parent().unwrap_or(Path::new(".")).to_path_buf());

            let file_type = InputFileType::from_path(&cmd.input)
                .ok_or(eyre::eyre!("Unsupported input file type"))?;
            match file_type {
                InputFileType::Bnk => {
                    SoundToolProject::dump_bnk(input, &output_root).context("Failed to dump bnk")?
                }
                InputFileType::Pck => {
                    SoundToolProject::dump_pck(input, &output_root).context("Failed to dump pck")?
                }
                other => eyre::bail!("Unsupported input file type: {:?}", other),
            };
        }
        Command::SoundToWem(cmd) => {
            if cmd.input.is_empty() {
                eyre::bail!("No input file specified.");
            }
            for input in &cmd.input {
                info!("Input: {}", input);
            }
            if let Some(output) = &cmd.output {
                info!("Output: {}", output);
            }
            if !cmd.wwise_console.is_empty() {
                info!("WwiseConsole: {}", cmd.wwise_console);
            }
            if let Some(ffmpeg) = &cmd.ffmpeg {
                info!("FFmpeg: {}", ffmpeg);
            }
            {
                // sync config with cli args
                let mut config = Config::global().lock();
                if let Some(ffmpeg) = &cmd.ffmpeg {
                    config.set_bin_config("ffmpeg", ffmpeg);
                }
                if !cmd.wwise_console.is_empty() {
                    config.set_bin_config("WwiseConsole", &cmd.wwise_console);
                }
            }

            let output_dir = cmd.output.as_ref().map(PathBuf::from).unwrap_or_else(|| {
                let first_file_dir = Path::new(&cmd.input[0]).parent().unwrap_or(Path::new("."));
                first_file_dir.to_path_buf()
            });
            // create temp dir
            let temp_dir = tempfile::tempdir()?;
            let temp_dir = temp_dir.path().join("sound2wem");
            if temp_dir.exists() {
                fs::remove_dir_all(&temp_dir)?;
                fs::create_dir_all(&temp_dir)?;
            } else {
                fs::create_dir_all(&temp_dir)?;
            }
            // transcode to wav in temp dir
            for input in &cmd.input {
                let input = Path::new(input);
                if !input.is_file() {
                    eyre::bail!("Input file not found: {}", input.display())
                }
                if input.extension().unwrap_or_default() == "wav" {
                    // copy to temp dir
                    let out_file = temp_dir.join(input.file_name().unwrap());
                    fs::copy(input, &out_file)?;
                } else {
                    // transcode to wav in temp dir
                    let mut data =
                        transcode::sounds_to_wav(&[input]).context("Failed to transcode to wav")?;
                    let data = data.pop().unwrap();
                    // 写入临时文件
                    let ff_out_file_name =
                        Path::new(input.file_stem().unwrap()).with_extension("wav");
                    let ff_out_file = temp_dir.join(ff_out_file_name);
                    fs::write(&ff_out_file, &data).context(format!(
                        "Failed to write transcoded data {}",
                        ff_out_file.display()
                    ))?;
                }
            }
            // to wem
            transcode::wavs_to_wem(&temp_dir, &output_dir)?;
        }
    }

    Ok(())
}

fn wait_for_exit() {
    if INTERACTIVE_MODE.load(atomic::Ordering::SeqCst) {
        let _: String = Input::new()
            .allow_empty(true)
            .with_prompt("Press Enter to exit")
            .interact()
            .unwrap();
    }
}
