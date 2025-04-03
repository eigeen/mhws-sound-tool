use std::{
    env, io,
    path::{Path, PathBuf},
    process::Command,
};

type Result<T> = std::result::Result<T, FFmpegError>;

#[derive(Debug, thiserror::Error)]
pub enum FFmpegError {
    #[error("Wwise module IO error: {0}")]
    IO(#[from] std::io::Error),

    #[error("ffmpeg executable not found.")]
    FFmpegNotFound,
    #[error("Command failed: {code:?}\n{stdout}\n{stderr}")]
    CommandFailed {
        code: Option<i32>,
        stdout: String,
        stderr: String,
    },
    #[error("Command execution failed: {0}")]
    CommandExecutionFailed(io::Error),
}

impl FFmpegError {
    fn command_failed(code: Option<i32>, stdout: &[u8], stderr: &[u8]) -> Self {
        FFmpegError::CommandFailed {
            code,
            stdout: String::from_utf8_lossy(stdout).to_string(),
            stderr: String::from_utf8_lossy(stderr).to_string(),
        }
    }
}

pub struct FFmpegCli {
    program_path: PathBuf,
}

impl FFmpegCli {
    pub fn new() -> Result<Self> {
        let mut try_paths = vec![];
        // env
        if let Ok(path) = env::var("FFMPEG_PATH") {
            try_paths.push(PathBuf::from(path));
        }
        // inside exe dir
        let exe_path = env::current_exe()?;
        let exe_dir = exe_path.parent().unwrap();
        try_paths.push(exe_dir.join("ffmpeg"));
        // inside cwd
        let cwd = env::current_dir()?;
        try_paths.push(cwd.join("ffmpeg"));
        // global
        try_paths.push(PathBuf::from("ffmpeg"));

        for path in try_paths {
            if Self::test_ffmpeg_cli(&path) {
                return Ok(Self { program_path: path });
            };
        }

        Err(FFmpegError::FFmpegNotFound)
    }

    pub fn new_with_path(program_path: PathBuf) -> Option<Self> {
        if !Self::test_ffmpeg_cli(&program_path) {
            return None;
        }
        Some(Self { program_path })
    }

    pub fn program_path(&self) -> &Path {
        self.program_path.as_ref()
    }

    /// Simple transcode, only provide input and output file path.
    pub fn simple_transcode(
        &self,
        input: impl AsRef<Path>,
        output: impl AsRef<Path>,
    ) -> Result<()> {
        let input = input.as_ref();
        let output = output.as_ref();

        let program_path: &Path = self.program_path.as_ref();
        let result = Command::new(program_path)
            .args([
                "-hide_banner",
                "-loglevel",
                "warning",
                "-i",
                input.to_str().unwrap(),
                "-y",
                output.to_str().unwrap(),
            ])
            .output()
            .map_err(FFmpegError::CommandExecutionFailed)?;

        if !result.status.success() {
            return Err(FFmpegError::command_failed(
                Some(result.status.code().unwrap()),
                &result.stdout,
                &result.stderr,
            ));
        }

        Ok(())
    }

    /// Test if the ffmpeg can be executed.
    fn test_ffmpeg_cli(program_path: impl AsRef<Path>) -> bool {
        let result = Command::new(program_path.as_ref())
            .args(["-version"])
            .output();
        let Ok(result) = result else {
            return false;
        };

        result.status.success()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ffmpeg_cli() {
        let _ffmpeg_cli = FFmpegCli::new().unwrap();
        eprintln!("path: {}", _ffmpeg_cli.program_path.display());
    }

    #[test]
    fn test_simple_transcode() {
        let ffmpeg_cli = FFmpegCli::new().unwrap();
        ffmpeg_cli
            .simple_transcode(
                "test_files/test_sound.mp3",
                "test_files/simple_transcode_output.wav",
            )
            .unwrap();
        assert!(Path::new("test_files/simple_transcode_output.wav").is_file());
    }
}
