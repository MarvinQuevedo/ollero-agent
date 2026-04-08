use std::process::Command;
use std::env;
use anyhow::Result;

#[derive(Debug, Default)]
pub struct Dependency {
    pub name: String,
    pub installed: bool,
    pub description: String,
    pub install_cmd: String,
}

#[derive(Debug, Default)]
pub struct DoctorReport {
    pub dependencies: Vec<Dependency>,
    pub os: String,
    pub platform_flags: Vec<String>,
}

pub struct Doctor;

impl Doctor {
    pub fn new() -> Self {
        Self
    }

    fn get_os() -> String {
        env::consts::OS.to_string()
    }

    fn command_exists(cmd: &str) -> bool {
        if cfg!(target_os = "windows") {
            Command::new("where")
                .arg(cmd)
                .output()
                .map(|output| output.status.success())
                .unwrap_or(false)
        } else {
            Command::new("which")
                .arg(cmd)
                .output()
                .map(|output| output.status.success())
                .unwrap_or(false)
        }
    }

    pub fn check_dependencies(&self) -> DoctorReport {
        let os = Self::get_os();
        let mut dependencies = Vec::new();
        let mut platform_flags = Vec::new();

        // Common dependencies
        dependencies.push(Dependency {
            name: "ollama".to_string(),
            installed: Self::command_exists("ollama"),
            description: "The Ollama engine for running LLMs".to_string(),
            install_cmd: "Download from https://ollama.com/".to_string(),
        });

        match os.as_str() {
            "macos" => {
                dependencies.push(Dependency {
                    name: "asitop".to_string(),
                    installed: Self::command_exists("asitop"),
                    description: "Apple Silicon performance monitor".to_string(),
                    install_cmd: "pip install asitop".to_string(),
                });
                
                if Self::command_exists("powermetrics") {
                    platform_flags.push("powermetrics is available for hardware monitoring".to_string());
                }
            }
            "linux" => {
                dependencies.push(Dependency {
                    name: "nvidia-smi".to_string(),
                    installed: Self::command_exists("nvidia-smi"),
                    description: "NVIDIA GPU monitoring tool".to_string(),
                    install_cmd: "sudo apt install nvidia-utils-<version>".to_string(),
                });
                
                dependencies.push(Dependency {
                    name: "htop".to_string(),
                    installed: Self::command_exists("htop"),
                    description: "System process monitor".to_string(),
                    install_cmd: "sudo apt install htop".to_string(),
                });
            }
            "windows" => {
                dependencies.push(Dependency {
                    name: "git".to_string(),
                    installed: Self::command_exists("git"),
                    description: "Version control system".to_string(),
                    install_cmd: "winget install Git.Git".to_string(),
                });
            }
            _ => {
                platform_flags.push(format!("Unsupported OS: {}", os));
            }
        }

        DoctorReport {
            dependencies,
            os,
            platform_flags,
        }
    }
}
