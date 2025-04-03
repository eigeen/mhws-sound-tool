use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic,
};

use dialoguer::{Input, theme::ColorfulTheme};
use eyre::Context;
use log::{debug, info, warn};

use crate::{
    INTERACTIVE_MODE,
    config::Config,
    ffmpeg::FFmpegCli,
    wwise::{WwiseConsole, WwiseSource},
};

/// Transcode all wav files in input_dir to wem files in output_dir.
pub fn wavs_to_wem(input_dir: impl AsRef<Path>, output_dir: impl AsRef<Path>) -> eyre::Result<()> {
    let input_dir = input_dir.as_ref().canonicalize().context(format!(
        "Failed to canonicalize input path: {}",
        input_dir.as_ref().display()
    ))?;
    let output_dir = output_dir.as_ref();

    // create wsource
    let mut source = WwiseSource::new(input_dir.to_str().unwrap());
    let read_dir = input_dir
        .read_dir()
        .context("Failed to read input directory")?;
    for entry in read_dir {
        let entry = entry.context("Failed to read input directory entry")?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        debug!("Add source: {}", path.display());
        source.add_source(path.to_str().unwrap());
    }
    // convert
    let wconsole = require_wwise_console()?;
    let wproject = wconsole.acquire_temp_project()?;
    wproject
        .convert_external_source(&source, output_dir.to_str().unwrap())
        .context("Failed to convert to wem")?;
    // mv to root
    let ww_output_dir = output_dir.join("Windows");
    if ww_output_dir.exists() {
        let read_dir = ww_output_dir
            .read_dir()
            .context("Failed to read output directory")?;
        for entry in read_dir {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let to = output_dir.join(path.file_name().unwrap());
            debug!("Output: {}", to.display());
            fs::copy(&path, to)?;
        }
        // remove ww_output_dir "Windows"
        let _ = fs::remove_dir_all(&ww_output_dir);
    }

    Ok(())
}

/// Transcode all sounds in inputs to wav files data.
pub fn sounds_to_wav(inputs: &[impl AsRef<Path>]) -> eyre::Result<Vec<Vec<u8>>> {
    let ffmpeg = require_ffmpeg()?;
    let tmp_dir = tempfile::tempdir()?;
    let mut wavs = vec![];
    for input in inputs {
        let input = input.as_ref();
        let file_stem = input.file_stem().unwrap().to_str().unwrap();
        let output_file_name = Path::new(file_stem).with_extension("wav");
        let output_path = tmp_dir.path().join(output_file_name);
        debug!("Transcoding: {}", input.display());
        ffmpeg.simple_transcode(input, &output_path)?;

        let output_data =
            fs::read(&output_path).context("Failed to read ffmpeg transcoded output file")?;
        wavs.push(output_data);
    }

    Ok(wavs)
}

/// Get ffmpeg instance from config, or update config with user input.
fn require_ffmpeg() -> eyre::Result<FFmpegCli> {
    let mut config = Config::global().lock();
    if let Some(ffmpeg_config) = config.get_bin_config("ffmpeg") {
        return FFmpegCli::new_with_path(PathBuf::from(&ffmpeg_config.path))
            .ok_or(eyre::eyre!("FFmpeg not found"));
    }
    if !crate::INTERACTIVE_MODE.load(atomic::Ordering::SeqCst) {
        eyre::bail!("ffmpeg path is not set, and interactive mode is disabled.");
    }

    warn!("ffmpeg path is not set, please setup in config.toml.");
    let ffmpeg_path: String = Input::with_theme(&ColorfulTheme::default())
        .show_default(true)
        .default("ffmpeg.exe".to_string())
        .with_prompt("Input ffmpeg path")
        .interact_text()
        .unwrap();
    let ffmpeg_path = ffmpeg_path.trim_matches(['\"', '\'']);
    let ffmpeg = FFmpegCli::new_with_path(PathBuf::from(ffmpeg_path))
        .ok_or(eyre::eyre!("FFmpeg not found"))?;
    config.set_bin_config("ffmpeg", ffmpeg.program_path().to_string_lossy().as_ref());
    config.save();
    info!("FFmpeg path saved to config.toml.");

    Ok(ffmpeg)
}

/// Get wwise console instance from config, or update config with user input.
fn require_wwise_console() -> eyre::Result<WwiseConsole> {
    let mut config = Config::global().lock();
    if let Some(wconsole_config) = config.get_bin_config("WwiseConsole") {
        return Ok(WwiseConsole::new_with_path(PathBuf::from(
            &wconsole_config.path,
        ))?);
    }
    if !INTERACTIVE_MODE.load(atomic::Ordering::SeqCst) {
        eyre::bail!("WwiseConsole path is not set, and interactive mode is disabled.");
    }

    warn!("WwiseConsole path is not set, please setup in config.toml.");
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
    info!("WwiseConsole path saved to config.toml.");

    Ok(wconsole)
}
