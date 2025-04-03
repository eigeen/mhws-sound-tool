use std::{
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::Command,
};

const WWISE_BASE_DEFAULT_PATH: &str = r"C:\Program Files (x86)\Audiokinetic";

type Result<T> = std::result::Result<T, WwiseError>;

#[derive(Debug, thiserror::Error)]
pub enum WwiseError {
    #[error("Wwise module IO error: {0}")]
    IO(#[from] std::io::Error),

    #[error("Wwise console not found.")]
    WwiseConsoleNotFound,
    #[error("Project already exists: {0}")]
    ProjectAlreadyExists(PathBuf),
    #[error("Command failed: {code:?}\n{stdout}\n{stderr}")]
    CommandFailed {
        code: Option<i32>,
        stdout: String,
        stderr: String,
    },
    #[error("Command execution failed: {0}")]
    CommandExecutionFailed(io::Error),
    #[error("Assertion failed: {0}")]
    Assertion(String),
}

impl WwiseError {
    fn command_failed(code: Option<i32>, stdout: &[u8], stderr: &[u8]) -> Self {
        WwiseError::CommandFailed {
            code,
            stdout: String::from_utf8_lossy(stdout).to_string(),
            stderr: String::from_utf8_lossy(stderr).to_string(),
        }
    }
}

#[derive(Default)]
pub struct WwiseConsole {
    console_path: PathBuf,
}

impl WwiseConsole {
    pub fn new() -> Result<Self> {
        if let Ok(root_path) = env::var("WWISEROOT") {
            let root_path = PathBuf::from(root_path);
            let console_path = root_path.join(r"Authoring\x64\Release\bin\WwiseConsole.exe");
            if console_path.exists() {
                if Self::test_console(&console_path) {
                    return Ok(Self { console_path });
                } else {
                    return Err(WwiseError::Assertion(format!(
                        "Found console but failed to test: {}",
                        console_path.display()
                    )));
                }
            }
        }

        // try to find in default path
        let wwise_base_path = PathBuf::from(WWISE_BASE_DEFAULT_PATH);
        if !wwise_base_path.exists() {
            return Err(WwiseError::WwiseConsoleNotFound);
        }

        let wwise_version_dirs = fs::read_dir(&wwise_base_path)?;
        let mut console_path = None;
        for entry in wwise_version_dirs {
            let entry = entry?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let path = path.join(r"Authoring\x64\Release\bin\WwiseConsole.exe");
            if path.exists() {
                console_path = Some(path);
                break;
            }
        }

        if let Some(path) = console_path {
            if Self::test_console(&path) {
                Ok(Self { console_path: path })
            } else {
                Err(WwiseError::Assertion(format!(
                    "Found console but failed to test: {}",
                    path.display()
                )))
            }
        } else {
            Err(WwiseError::WwiseConsoleNotFound)
        }
    }

    pub fn new_with_path(console_path: impl AsRef<Path>) -> Result<Self> {
        let console_path = console_path.as_ref().to_path_buf();
        if !console_path.exists() {
            return Err(WwiseError::WwiseConsoleNotFound);
        }
        if !Self::test_console(&console_path) {
            return Err(WwiseError::Assertion(format!(
                "Found console but failed to test: {}",
                console_path.display()
            )));
        }

        Ok(Self { console_path })
    }

    pub fn program_path(&self) -> &Path {
        &self.console_path
    }

    pub fn acquire_temp_project(&self) -> Result<WwiseProject> {
        const TEMP_PROJECT_NAME: &str = "SoundToolTemp";

        let exe_path = env::current_exe()?;
        let tool_dir = exe_path.parent().unwrap();
        let proj_path = tool_dir
            .join(TEMP_PROJECT_NAME)
            .join(format!("{}.wproj", TEMP_PROJECT_NAME));
        if proj_path.exists() {
            let project = WwiseProject::new(self, proj_path);
            return Ok(project);
        }

        // not exist, try to create the project
        let project = self.create_new_project(tool_dir, TEMP_PROJECT_NAME)?;
        Ok(project)
    }

    pub fn create_new_project(
        &self,
        root_path: impl AsRef<Path>,
        project_name: impl AsRef<str>,
    ) -> Result<WwiseProject> {
        let root_path = root_path.as_ref();
        let project_name = project_name.as_ref();
        if !root_path.exists() {
            fs::create_dir_all(root_path)?;
        }

        let project_path = root_path
            .join(project_name)
            .join(format!("{}.wproj", project_name));
        if project_path.exists() {
            return Err(WwiseError::ProjectAlreadyExists(project_path));
        }

        let result = Command::new(&self.console_path)
            .args([
                "create-new-project",
                project_path.to_str().unwrap(),
                "--platform",
                "Windows",
            ])
            .output()
            .map_err(WwiseError::CommandExecutionFailed)?;
        if !result.status.success() {
            return Err(WwiseError::command_failed(
                result.status.code(),
                &result.stdout,
                &result.stderr,
            ));
        }

        // check if the project exists
        if !project_path.exists() {
            return Err(WwiseError::Assertion(format!(
                "Project not exists after creation: {}",
                project_path.display()
            )));
        }
        Ok(WwiseProject::new(self, project_path))
    }

    /// Test if the console can be executed.
    fn test_console(console_path: impl AsRef<Path>) -> bool {
        let result = Command::new(console_path.as_ref())
            .args(["create-new-project", "--help"])
            .output();
        let Ok(result) = result else {
            return false;
        };

        result.status.success()
    }
}

pub struct WwiseProject<'a> {
    console: &'a WwiseConsole,
    project_path: PathBuf,
}

impl<'a> WwiseProject<'a> {
    fn new(console: &'a WwiseConsole, project_path: PathBuf) -> Self {
        Self {
            console,
            project_path,
        }
    }

    #[allow(dead_code)]
    pub fn project_path(&self) -> &Path {
        &self.project_path
    }

    pub fn convert_external_source(
        &self,
        wsource: &WwiseSource,
        output_dir: impl AsRef<str>,
    ) -> Result<()> {
        let xml = wsource.to_xml();
        // write to temp file
        let source_file_name = "list.wsource";
        let source_file_path = self.project_path.parent().unwrap().join(source_file_name);
        {
            let mut file = fs::File::create(&source_file_path)?;
            file.write_all(xml.as_bytes())?;
        }

        let output_path = output_dir.as_ref().replace("/", "\\").replace(r"\\?\", "");
        let result = Command::new(&self.console.console_path)
            .args([
                "convert-external-source",
                self.project_path.to_str().unwrap(),
                "--source-file",
                source_file_path.to_str().unwrap(),
                "--output",
                &output_path,
            ])
            .output()
            .map_err(WwiseError::CommandExecutionFailed)?;
        if !result.status.success() {
            return Err(WwiseError::command_failed(
                result.status.code(),
                &result.stdout,
                &result.stderr,
            ));
        }

        // TODO: check if the converted source exists
        Ok(())
    }
}

pub struct WwiseSource {
    root: String,
    sources: Vec<String>,
}

impl WwiseSource {
    pub fn new(root: impl AsRef<str>) -> Self {
        let root = root.as_ref().replace("/", "\\").replace(r"\\?\", "");
        Self {
            root,
            sources: vec![],
        }
    }

    pub fn add_source(&mut self, source: impl AsRef<str>) {
        let source = source.as_ref().replace("/", "\\").replace(r"\\?\", "");
        self.sources.push(source);
    }

    fn to_xml(&self) -> String {
        let mut sources = String::new();
        for source in self.sources.iter() {
            sources += &format!(
                "    <Source Path=\"{}\" Conversion=\"Vorbis Quality High\"/>\n",
                source
            );
        }
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<ExternalSourcesList SchemaVersion="1" Root="{root}">
{sources}
</ExternalSourcesList>"#,
            root = self.root,
            sources = sources
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_console() {
        let _console = WwiseConsole::new().unwrap();
    }

    #[test]
    fn test_acquire_temp_project() {
        let console = WwiseConsole::new().unwrap();
        let project = console.acquire_temp_project().unwrap();
        assert!(project.project_path.exists());
    }

    #[test]
    fn test_convert() {
        let console = WwiseConsole::new().unwrap();
        let root = env::current_dir().unwrap().join("test_files");
        let root_str = root.to_str().unwrap();
        let project = console.acquire_temp_project().unwrap();
        let mut source = WwiseSource::new(root_str);
        source.add_source("test_sound.wav");
        project.convert_external_source(&source, root_str).unwrap();
    }
}
