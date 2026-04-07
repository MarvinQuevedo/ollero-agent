# Allux Agent 🚀

> **Note:** This README and parts of this project were created by **Allux**, an AI agent, using the **Allux** tool itself.

## 🚀 Introduction
Allux is a local code agent built with Rust, designed to assist in software development tasks by integrating with Ollama. This project combines asynchronous processing, file manipulation, and modular extensibility to offer an efficient and powerful development experience.

---

## 📋 Table of Contents
1. [📌 Overview](#overview)
2. [🔧 Architecture and Technologies](#architecture-and-technologies)
3. [📂 Project Structure](#project-structure)
4. [🛠 Tools and Dependencies](#tools-and-dependencies)
5. [🚀 Installation and Setup](#installation-and-setup)
6. [🧪 Basic Usage](#basic-usage)
7. [🔧 Extensibility with Skills](#extensibility-with-skills)
8. [📝 Contributions](#contributions)
9. [📄 Additional Documentation](#additional-documentation)
10. [📋 Licenses](#licenses)

---

## 📌 Overview
Allux is built on:
- **Tokio** for asynchronous operations.
- **Rust** for performance and security.
- **Ollama** for language model integration.

The project allows exploring, modifying, and executing code locally with advanced file processing and pattern matching capabilities.

---

## 🔧 Architecture and Technologies

### 🔄 Architecture
- **Modular**: Each functionality is encapsulated in independent modules.
- **Asynchronous**: Designed to handle multiple tasks simultaneously.
- **Extensible**: Skill system for adding specific functionalities.

### 🛠 Key Technologies
| Technology      | Purpose                                                                 |
|-----------------|-------------------------------------------------------------------------|
| Rust            | Main language for performance and security.                             |
| Tokio           | Asynchronous runtime for I/O handling.                                  |
| Reqwest         | HTTP client with streaming support.                                     |
| Serde           | JSON serialization/deserialization.                                     |
| Crossterm       | Terminal input/output handling.                                         |
| Glob            | File pattern searching.                                                 |
| Regex           | Text pattern processing.                                                |
| Pulldown-cmark  | Markdown processing.                                                    |
| Indicatif       | Progress bars and load indicators.                                      |

---

## 📂 Project Structure
```
allux-agent/
├── Cargo.toml          # Project dependencies and configuration.
├── README.md           # Main documentation.
├── LICENSE             # Project license.
├── docs/               # Technical documentation and guides.
├── scripts/            # Utility scripts.
├── src/                # Main source code.
│   ├── main.rs         # Program entry and REPL.
│   ├── ollama/         # Ollama client and types.
│   ├── tools/          # Built-in tools (bash, grep, edit, etc.).
│   └── ...             # Other modules (config, session, etc.).
├── tests/              # Integration and unit tests.
├── validation/         # Test prompts and validation data.
└── skills-lock.json    # Skills dependencies.
```

---

## 🛠 Tools and Dependencies

### 📦 Main Dependencies
| Dependency   | Version | Purpose                                  |
|--------------|---------|------------------------------------------|
| reqwest      | 0.12    | HTTP client with streaming.              |
| tokio        | 1.0     | Asynchronous runtime.                    |
| serde        | 1.0     | JSON serialization.                     |
| crossterm    | 0.28    | Terminal input/output.                  |
| glob         | 0.3     | File searching.                         |
| regex        | 1.0     | Pattern processing.                     |
| pulldown-cmark| 0.12    | Markdown processing.                    |
| indicatif    | 0.17    | Progress bars.                          |

---

## 🚀 Installation and Setup

### 📦 Prerequisites
1. **Rust**: Install Rust toolchain from [rustup.rs](https://rustup.rs).
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```

2. **Ollama**: Install Ollama for language model integration.
   ```bash
   curl -fsSL https://ollama.com/install.sh | sh
   ```

### 🛠 Project Setup
1. Clone the repository:
   ```bash
   git clone https://github.com/usuario/allux-agent.git
   cd allux-agent
   ```

2. Build the project:
   ```bash
   cargo build
   ```

3. Run the agent:
   ```bash
   cargo run --bin allux
   ```

---

## 🧪 Basic Usage

### 📌 Main Capabilities
| Command/Tool       | Description                                  |
|-----------------------|----------------------------------------------|
| `bash`                | Execute shell commands.                      |
| `grep <pattern>`      | Search patterns in files.                  |
| `read_file <path>`    | Read file contents.                         |
| `write_file <path>`   | Write/Overwrite files.                      |
| `edit_file <path>`    | Edit specific strings in files.             |
| `tree <path>`         | Display directory structure.                |

---

## 🔧 Extensibility with Skills
Allux allows adding specific functionalities using the skill system. Each skill is an independent module that can add new capabilities to the agent.

### 📦 Installing Skills
1. **Install a skill**:
   ```bash
   npx --yes skills add <owner/repo> --skill <name> -y
   ```

---

## 📝 Contributions
1. **Clone the repository**.
2. **Create a branch** for your feature.
3. **Write tests** to ensure stability.
4. **Submit a Pull Request**.

---

## 📄 Additional Documentation
- **[Architecture Docs](docs/architecture/overview.md)**: Detailed technical design.
- **[Guides](docs/guides/index.md)**: How to use and configure Allux.

---

## 📋 Licenses
This project is licensed under the [GPL-3.0-or-later](LICENSE).
