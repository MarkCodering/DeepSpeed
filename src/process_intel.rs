/// Process Intelligence — AI-powered process identification and description.
/// Combines a built-in knowledge base of ~200 common processes with AI fallback
/// for unknown processes. Enables the TUI to show users what each process does.
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Mutex;

/// A description of what a process does, its category, and resource profile.
#[derive(Debug, Clone)]
pub struct ProcessDescription {
    pub short: &'static str,
    pub category: ProcessCategory,
    pub resource_profile: ResourceProfile,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ProcessCategory {
    System,
    Browser,
    DevTool,
    Media,
    Communication,
    Editor,
    Server,
    AI,
    Database,
    Container,
    Security,
    Desktop,
    FileSystem,
    Network,
    Gaming,
    Productivity,
    Unknown,
}

impl ProcessCategory {
    pub fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Browser => "Browser",
            Self::DevTool => "Dev Tool",
            Self::Media => "Media",
            Self::Communication => "Comms",
            Self::Editor => "Editor",
            Self::Server => "Server",
            Self::AI => "AI/ML",
            Self::Database => "Database",
            Self::Container => "Container",
            Self::Security => "Security",
            Self::Desktop => "Desktop",
            Self::FileSystem => "Filesystem",
            Self::Network => "Network",
            Self::Gaming => "Gaming",
            Self::Productivity => "Productivity",
            Self::Unknown => "Unknown",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Self::System => "SYS",
            Self::Browser => "WEB",
            Self::DevTool => "DEV",
            Self::Media => "MED",
            Self::Communication => "COM",
            Self::Editor => "EDT",
            Self::Server => "SRV",
            Self::AI => "AI ",
            Self::Database => "DB ",
            Self::Container => "CTR",
            Self::Security => "SEC",
            Self::Desktop => "DSK",
            Self::FileSystem => "FS ",
            Self::Network => "NET",
            Self::Gaming => "GAM",
            Self::Productivity => "PRD",
            Self::Unknown => "???",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ResourceProfile {
    /// Typically low resource usage
    Light,
    /// Moderate, steady resource usage
    Moderate,
    /// Can consume significant CPU/RAM
    Heavy,
    /// Known to spike resources unpredictably
    Bursty,
}

impl ResourceProfile {
    pub fn label(self) -> &'static str {
        match self {
            Self::Light => "Light",
            Self::Moderate => "Moderate",
            Self::Heavy => "Heavy",
            Self::Bursty => "Bursty",
        }
    }
}

/// Cache for AI-generated descriptions of unknown processes.
static AI_CACHE: Lazy<Mutex<HashMap<String, String>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Built-in knowledge base of common processes.
static KNOWN_PROCESSES: Lazy<HashMap<&'static str, ProcessDescription>> = Lazy::new(|| {
    let mut m = HashMap::new();
    let ins = |m: &mut HashMap<&'static str, ProcessDescription>,
               names: &[&'static str],
               short: &'static str,
               cat: ProcessCategory,
               prof: ResourceProfile| {
        let desc = ProcessDescription { short, category: cat, resource_profile: prof };
        for &n in names {
            m.insert(n, desc.clone());
        }
    };

    // ── System / Kernel ──────────────────────────────────────────────
    ins(&mut m, &["kernel_task", "kthreadd", "System"], "OS kernel scheduler & threads", ProcessCategory::System, ResourceProfile::Light);
    ins(&mut m, &["launchd", "systemd", "init"], "System init / service manager", ProcessCategory::System, ResourceProfile::Light);
    ins(&mut m, &["loginwindow", "winlogon.exe"], "User login session manager", ProcessCategory::System, ResourceProfile::Light);
    ins(&mut m, &["WindowServer"], "macOS display compositor", ProcessCategory::Desktop, ResourceProfile::Moderate);
    ins(&mut m, &["Dock"], "macOS app launcher & dock", ProcessCategory::Desktop, ResourceProfile::Light);
    ins(&mut m, &["Finder", "explorer.exe", "nautilus", "dolphin", "thunar"], "File manager", ProcessCategory::Desktop, ResourceProfile::Moderate);
    ins(&mut m, &["SystemUIServer"], "macOS menu bar & system UI", ProcessCategory::Desktop, ResourceProfile::Light);
    ins(&mut m, &["mds", "mds_stores", "mdworker"], "Spotlight search indexer", ProcessCategory::FileSystem, ResourceProfile::Bursty);
    ins(&mut m, &["configd"], "macOS system configuration daemon", ProcessCategory::System, ResourceProfile::Light);
    ins(&mut m, &["notifyd"], "macOS notification center daemon", ProcessCategory::System, ResourceProfile::Light);
    ins(&mut m, &["coreaudiod", "audiodg.exe", "pulseaudio", "pipewire"], "Audio subsystem daemon", ProcessCategory::Media, ResourceProfile::Light);
    ins(&mut m, &["bluetoothd", "bluetoothctl"], "Bluetooth service daemon", ProcessCategory::System, ResourceProfile::Light);
    ins(&mut m, &["svchost.exe"], "Windows shared service host", ProcessCategory::System, ResourceProfile::Moderate);
    ins(&mut m, &["dwm.exe"], "Windows desktop compositor (DWM)", ProcessCategory::Desktop, ResourceProfile::Moderate);
    ins(&mut m, &["csrss.exe"], "Windows client/server runtime", ProcessCategory::System, ResourceProfile::Light);
    ins(&mut m, &["lsass.exe"], "Windows auth & security service", ProcessCategory::Security, ResourceProfile::Light);
    ins(&mut m, &["smss.exe"], "Windows session manager", ProcessCategory::System, ResourceProfile::Light);
    ins(&mut m, &["wininit.exe"], "Windows initialization process", ProcessCategory::System, ResourceProfile::Light);
    ins(&mut m, &["services.exe"], "Windows service control manager", ProcessCategory::System, ResourceProfile::Light);
    ins(&mut m, &["gnome-shell", "plasmashell", "mutter"], "Desktop environment compositor", ProcessCategory::Desktop, ResourceProfile::Moderate);
    ins(&mut m, &["Xorg", "Xwayland"], "X Window System display server", ProcessCategory::Desktop, ResourceProfile::Moderate);
    ins(&mut m, &["wireplumber"], "PipeWire session/policy manager", ProcessCategory::Media, ResourceProfile::Light);
    ins(&mut m, &["NetworkManager", "networkd"], "Network connection manager", ProcessCategory::Network, ResourceProfile::Light);
    ins(&mut m, &["dbus-daemon", "dbus-broker"], "D-Bus inter-process message bus", ProcessCategory::System, ResourceProfile::Light);
    ins(&mut m, &["polkitd"], "Authorization policy daemon", ProcessCategory::Security, ResourceProfile::Light);
    ins(&mut m, &["cron", "crond", "anacron"], "Scheduled task runner", ProcessCategory::System, ResourceProfile::Light);
    ins(&mut m, &["journald", "rsyslogd", "syslogd"], "System log collection daemon", ProcessCategory::System, ResourceProfile::Light);
    ins(&mut m, &["udevd", "systemd-udevd"], "Device manager for Linux kernel", ProcessCategory::System, ResourceProfile::Light);
    ins(&mut m, &["thermald"], "CPU thermal management daemon", ProcessCategory::System, ResourceProfile::Light);
    ins(&mut m, &["irqbalance"], "IRQ balancer across CPU cores", ProcessCategory::System, ResourceProfile::Light);
    ins(&mut m, &["kworker", "ksoftirqd", "rcu_sched"], "Kernel worker thread", ProcessCategory::System, ResourceProfile::Light);

    // ── Browsers ─────────────────────────────────────────────────────
    ins(&mut m, &["Google Chrome", "chrome", "chrome.exe", "Chromium"], "Google Chrome web browser", ProcessCategory::Browser, ResourceProfile::Heavy);
    ins(&mut m, &["Google Chrome Helper", "chrome_crashpad_handler"], "Chrome renderer / helper process", ProcessCategory::Browser, ResourceProfile::Heavy);
    ins(&mut m, &["firefox", "firefox.exe", "Firefox"], "Firefox web browser", ProcessCategory::Browser, ResourceProfile::Heavy);
    ins(&mut m, &["Safari", "com.apple.WebKit.WebContent"], "Safari / WebKit renderer", ProcessCategory::Browser, ResourceProfile::Heavy);
    ins(&mut m, &["com.apple.WebKit.Networking"], "Safari network subsystem", ProcessCategory::Browser, ResourceProfile::Moderate);
    ins(&mut m, &["Microsoft Edge", "msedge.exe", "msedge"], "Microsoft Edge browser", ProcessCategory::Browser, ResourceProfile::Heavy);
    ins(&mut m, &["Opera", "opera.exe", "Brave Browser", "brave"], "Chromium-based browser", ProcessCategory::Browser, ResourceProfile::Heavy);
    ins(&mut m, &["Arc", "Arc Helper"], "Arc browser (Chromium-based)", ProcessCategory::Browser, ResourceProfile::Heavy);

    // ── Dev Tools ────────────────────────────────────────────────────
    ins(&mut m, &["node", "node.exe", "nodejs"], "Node.js JavaScript runtime", ProcessCategory::DevTool, ResourceProfile::Bursty);
    ins(&mut m, &["python", "python3", "python.exe", "Python"], "Python interpreter", ProcessCategory::DevTool, ResourceProfile::Bursty);
    ins(&mut m, &["ruby", "ruby.exe"], "Ruby interpreter", ProcessCategory::DevTool, ResourceProfile::Moderate);
    ins(&mut m, &["java", "java.exe"], "Java Virtual Machine", ProcessCategory::DevTool, ResourceProfile::Heavy);
    ins(&mut m, &["javac", "javac.exe"], "Java compiler", ProcessCategory::DevTool, ResourceProfile::Heavy);
    ins(&mut m, &["cargo", "rustc", "rust-analyzer"], "Rust build tool / compiler / LSP", ProcessCategory::DevTool, ResourceProfile::Heavy);
    ins(&mut m, &["go", "gopls"], "Go compiler / language server", ProcessCategory::DevTool, ResourceProfile::Bursty);
    ins(&mut m, &["gcc", "g++", "cc1", "cc1plus", "clang", "clang++"], "C/C++ compiler", ProcessCategory::DevTool, ResourceProfile::Heavy);
    ins(&mut m, &["make", "cmake", "ninja"], "Build system orchestrator", ProcessCategory::DevTool, ResourceProfile::Bursty);
    ins(&mut m, &["ld", "lld", "gold", "mold"], "Binary linker", ProcessCategory::DevTool, ResourceProfile::Heavy);
    ins(&mut m, &["npm", "yarn", "pnpm", "bun"], "JavaScript package manager", ProcessCategory::DevTool, ResourceProfile::Bursty);
    ins(&mut m, &["pip", "pip3", "conda"], "Python package manager", ProcessCategory::DevTool, ResourceProfile::Moderate);
    ins(&mut m, &["git", "git-remote-https"], "Git version control", ProcessCategory::DevTool, ResourceProfile::Light);
    ins(&mut m, &["gh"], "GitHub CLI tool", ProcessCategory::DevTool, ResourceProfile::Light);
    ins(&mut m, &["tsc", "tsx", "esbuild", "swc", "webpack"], "JavaScript/TypeScript bundler", ProcessCategory::DevTool, ResourceProfile::Bursty);
    ins(&mut m, &["dotnet", "dotnet.exe"], ".NET runtime / SDK", ProcessCategory::DevTool, ResourceProfile::Heavy);
    ins(&mut m, &["swift", "swiftc", "swift-frontend"], "Swift compiler", ProcessCategory::DevTool, ResourceProfile::Heavy);
    ins(&mut m, &["kotlinc", "kotlin"], "Kotlin compiler / runtime", ProcessCategory::DevTool, ResourceProfile::Heavy);
    ins(&mut m, &["gradle", "gradlew", "mvn", "maven"], "JVM build system", ProcessCategory::DevTool, ResourceProfile::Heavy);
    ins(&mut m, &["lldb", "gdb", "lldb-server"], "Debugger", ProcessCategory::DevTool, ResourceProfile::Moderate);

    // ── Editors / IDEs ───────────────────────────────────────────────
    ins(&mut m, &["code", "Code Helper", "Code Helper (Renderer)", "Code - Insiders"], "Visual Studio Code editor", ProcessCategory::Editor, ResourceProfile::Heavy);
    ins(&mut m, &["Electron", "electron"], "Electron framework app", ProcessCategory::Editor, ResourceProfile::Heavy);
    ins(&mut m, &["idea", "idea.exe", "phpstorm", "webstorm", "goland", "pycharm", "rubymine", "clion", "rider", "datagrip"], "JetBrains IDE", ProcessCategory::Editor, ResourceProfile::Heavy);
    ins(&mut m, &["vim", "nvim", "neovim"], "Vim/Neovim text editor", ProcessCategory::Editor, ResourceProfile::Light);
    ins(&mut m, &["emacs"], "Emacs text editor", ProcessCategory::Editor, ResourceProfile::Moderate);
    ins(&mut m, &["sublime_text", "subl"], "Sublime Text editor", ProcessCategory::Editor, ResourceProfile::Light);
    ins(&mut m, &["Xcode", "XCBBuildService", "IBDesignablesAgent"], "Apple Xcode IDE / build service", ProcessCategory::Editor, ResourceProfile::Heavy);
    ins(&mut m, &["zed", "Zed"], "Zed editor (GPU-accelerated)", ProcessCategory::Editor, ResourceProfile::Moderate);
    ins(&mut m, &["cursor", "Cursor", "Cursor Helper"], "Cursor AI code editor", ProcessCategory::Editor, ResourceProfile::Heavy);

    // ── Communication ────────────────────────────────────────────────
    ins(&mut m, &["Slack", "slack", "slack.exe", "Slack Helper"], "Slack messaging (Electron)", ProcessCategory::Communication, ResourceProfile::Heavy);
    ins(&mut m, &["Discord", "discord", "discord.exe", "Discord Helper"], "Discord chat (Electron)", ProcessCategory::Communication, ResourceProfile::Heavy);
    ins(&mut m, &["zoom.us", "zoom", "Zoom", "zoom.exe"], "Zoom video conferencing", ProcessCategory::Communication, ResourceProfile::Heavy);
    ins(&mut m, &["Microsoft Teams", "Teams", "ms-teams", "teams.exe"], "Microsoft Teams", ProcessCategory::Communication, ResourceProfile::Heavy);
    ins(&mut m, &["Telegram", "telegram-desktop"], "Telegram messenger", ProcessCategory::Communication, ResourceProfile::Moderate);
    ins(&mut m, &["Signal", "signal-desktop"], "Signal encrypted messenger", ProcessCategory::Communication, ResourceProfile::Moderate);
    ins(&mut m, &["Mail", "thunderbird", "Outlook", "OUTLOOK.EXE"], "Email client", ProcessCategory::Communication, ResourceProfile::Moderate);
    ins(&mut m, &["Messages", "iMessage"], "Apple Messages / iMessage", ProcessCategory::Communication, ResourceProfile::Light);

    // ── Media ────────────────────────────────────────────────────────
    ins(&mut m, &["Spotify", "spotify", "spotify.exe"], "Spotify music streaming", ProcessCategory::Media, ResourceProfile::Moderate);
    ins(&mut m, &["Music", "iTunes", "iTunes.exe"], "Apple Music / iTunes", ProcessCategory::Media, ResourceProfile::Moderate);
    ins(&mut m, &["VLC", "vlc"], "VLC media player", ProcessCategory::Media, ResourceProfile::Moderate);
    ins(&mut m, &["mpv"], "mpv media player", ProcessCategory::Media, ResourceProfile::Moderate);
    ins(&mut m, &["ffmpeg", "ffprobe"], "FFmpeg media encoder/decoder", ProcessCategory::Media, ResourceProfile::Heavy);
    ins(&mut m, &["HandBrakeCLI", "HandBrake", "ghb"], "HandBrake video transcoder", ProcessCategory::Media, ResourceProfile::Heavy);
    ins(&mut m, &["obs", "obs-studio", "OBS"], "OBS Studio streaming/recording", ProcessCategory::Media, ResourceProfile::Heavy);
    ins(&mut m, &["Photos", "shotwell", "darktable"], "Photo management app", ProcessCategory::Media, ResourceProfile::Moderate);
    ins(&mut m, &["Photoshop", "photoshop.exe", "GIMP", "gimp"], "Image editor", ProcessCategory::Media, ResourceProfile::Heavy);
    ins(&mut m, &["Premiere Pro", "DaVinci Resolve"], "Video editor", ProcessCategory::Media, ResourceProfile::Heavy);

    // ── AI / ML ──────────────────────────────────────────────────────
    ins(&mut m, &["ollama", "ollama_llama_server"], "Ollama local LLM runtime", ProcessCategory::AI, ResourceProfile::Heavy);
    ins(&mut m, &["llama-server", "llama.cpp"], "llama.cpp inference server", ProcessCategory::AI, ResourceProfile::Heavy);
    ins(&mut m, &["deepspeed"], "DeepSpeed AI system optimizer", ProcessCategory::AI, ResourceProfile::Light);
    ins(&mut m, &["jupyter", "jupyter-notebook", "jupyter-lab"], "Jupyter notebook server", ProcessCategory::AI, ResourceProfile::Moderate);
    ins(&mut m, &["tensorboard"], "TensorFlow visualization server", ProcessCategory::AI, ResourceProfile::Moderate);
    ins(&mut m, &["nvidia-smi", "nvidia-persistenced"], "NVIDIA GPU management", ProcessCategory::AI, ResourceProfile::Light);
    ins(&mut m, &["cuda_memcheck"], "CUDA memory checker", ProcessCategory::AI, ResourceProfile::Heavy);
    ins(&mut m, &["tritonserver"], "NVIDIA Triton inference server", ProcessCategory::AI, ResourceProfile::Heavy);
    ins(&mut m, &["vllm", "text-generation-launcher"], "LLM serving engine", ProcessCategory::AI, ResourceProfile::Heavy);
    ins(&mut m, &["stable-diffusion", "ComfyUI"], "Image generation model", ProcessCategory::AI, ResourceProfile::Heavy);

    // ── Databases ────────────────────────────────────────────────────
    ins(&mut m, &["postgres", "postgresql", "pg_ctl"], "PostgreSQL database server", ProcessCategory::Database, ResourceProfile::Heavy);
    ins(&mut m, &["mysqld", "mariadbd", "mysql"], "MySQL/MariaDB database", ProcessCategory::Database, ResourceProfile::Heavy);
    ins(&mut m, &["mongod", "mongos"], "MongoDB NoSQL database", ProcessCategory::Database, ResourceProfile::Heavy);
    ins(&mut m, &["redis-server", "redis-cli"], "Redis in-memory data store", ProcessCategory::Database, ResourceProfile::Moderate);
    ins(&mut m, &["sqlite3"], "SQLite database CLI", ProcessCategory::Database, ResourceProfile::Light);
    ins(&mut m, &["clickhouse-server", "clickhouse"], "ClickHouse analytics database", ProcessCategory::Database, ResourceProfile::Heavy);
    ins(&mut m, &["elasticsearch", "opensearch"], "Elasticsearch search engine", ProcessCategory::Database, ResourceProfile::Heavy);

    // ── Servers / Infrastructure ─────────────────────────────────────
    ins(&mut m, &["nginx", "httpd", "apache2"], "Web server (HTTP reverse proxy)", ProcessCategory::Server, ResourceProfile::Moderate);
    ins(&mut m, &["caddy"], "Caddy web server (auto HTTPS)", ProcessCategory::Server, ResourceProfile::Moderate);
    ins(&mut m, &["sshd", "ssh", "ssh-agent"], "SSH daemon / client / agent", ProcessCategory::Server, ResourceProfile::Light);
    ins(&mut m, &["containerd", "containerd-shim"], "Container runtime daemon", ProcessCategory::Container, ResourceProfile::Moderate);
    ins(&mut m, &["dockerd", "docker", "Docker Desktop"], "Docker container engine", ProcessCategory::Container, ResourceProfile::Heavy);
    ins(&mut m, &["com.docker.backend", "com.docker.vmnetd"], "Docker Desktop backend service", ProcessCategory::Container, ResourceProfile::Heavy);
    ins(&mut m, &["kubelet", "kubectl", "kube-apiserver"], "Kubernetes node/control plane", ProcessCategory::Container, ResourceProfile::Heavy);
    ins(&mut m, &["podman"], "Podman container manager", ProcessCategory::Container, ResourceProfile::Moderate);
    ins(&mut m, &["etcd"], "Distributed key-value store", ProcessCategory::Server, ResourceProfile::Moderate);
    ins(&mut m, &["consul", "vault"], "HashiCorp service mesh/secrets", ProcessCategory::Server, ResourceProfile::Moderate);
    ins(&mut m, &["prometheus", "grafana-server", "grafana"], "Monitoring / observability stack", ProcessCategory::Server, ResourceProfile::Moderate);

    // ── Productivity ─────────────────────────────────────────────────
    ins(&mut m, &["Notion", "notion", "Notion Helper"], "Notion workspace (Electron)", ProcessCategory::Productivity, ResourceProfile::Heavy);
    ins(&mut m, &["Obsidian", "obsidian"], "Obsidian notes (Electron)", ProcessCategory::Productivity, ResourceProfile::Moderate);
    ins(&mut m, &["1Password", "op", "1password"], "1Password password manager", ProcessCategory::Security, ResourceProfile::Moderate);
    ins(&mut m, &["Bitwarden", "bitwarden"], "Bitwarden password manager", ProcessCategory::Security, ResourceProfile::Moderate);
    ins(&mut m, &["Preview", "evince", "okular"], "Document/PDF viewer", ProcessCategory::Productivity, ResourceProfile::Light);
    ins(&mut m, &["Numbers", "Pages", "Keynote"], "Apple iWork suite app", ProcessCategory::Productivity, ResourceProfile::Moderate);
    ins(&mut m, &["EXCEL.EXE", "WINWORD.EXE", "POWERPNT.EXE", "libreoffice"], "Office productivity suite", ProcessCategory::Productivity, ResourceProfile::Heavy);

    // ── Gaming ───────────────────────────────────────────────────────
    ins(&mut m, &["steam", "Steam", "steamwebhelper"], "Steam game platform", ProcessCategory::Gaming, ResourceProfile::Heavy);
    ins(&mut m, &["EpicGamesLauncher", "EpicWebHelper"], "Epic Games launcher", ProcessCategory::Gaming, ResourceProfile::Heavy);

    // ── Security / VPN ───────────────────────────────────────────────
    ins(&mut m, &["openvpn", "wireguard-go", "wg-quick"], "VPN client daemon", ProcessCategory::Security, ResourceProfile::Light);
    ins(&mut m, &["CrowdStrike", "falcon-sensor", "csagent"], "CrowdStrike endpoint security", ProcessCategory::Security, ResourceProfile::Moderate);
    ins(&mut m, &["clamd", "freshclam"], "ClamAV antivirus daemon", ProcessCategory::Security, ResourceProfile::Moderate);
    ins(&mut m, &["MRT", "XProtect"], "macOS malware removal / XProtect", ProcessCategory::Security, ResourceProfile::Bursty);
    ins(&mut m, &["firewalld", "ufw"], "Firewall management daemon", ProcessCategory::Security, ResourceProfile::Light);

    m
});

/// Look up a process name in the knowledge base.
/// Returns a static description if known, otherwise generates one heuristically.
pub fn describe_process(name: &str) -> (String, ProcessCategory, ResourceProfile) {
    // Exact match
    if let Some(desc) = KNOWN_PROCESSES.get(name) {
        return (desc.short.to_string(), desc.category, desc.resource_profile);
    }

    // Case-insensitive match
    for (&key, desc) in KNOWN_PROCESSES.iter() {
        if key.eq_ignore_ascii_case(name) {
            return (desc.short.to_string(), desc.category, desc.resource_profile);
        }
    }

    // Check AI cache
    if let Ok(cache) = AI_CACHE.lock() {
        if let Some(cached) = cache.get(name) {
            return (cached.clone(), ProcessCategory::Unknown, ResourceProfile::Moderate);
        }
    }

    // Heuristic description based on name patterns
    let lower = name.to_lowercase();
    let (desc, cat, prof) = heuristic_classify(&lower, name);

    // Cache the heuristic result
    if let Ok(mut cache) = AI_CACHE.lock() {
        cache.insert(name.to_string(), desc.clone());
    }

    (desc, cat, prof)
}

/// Generate AI prompt context for unknown processes (used by TUI detail view).
#[allow(dead_code)]
pub fn ai_describe_prompt(name: &str, exe_path: Option<&str>, memory_mb: u64, cpu_pct: f32) -> String {
    format!(
        "Identify this running process in ONE short sentence (max 12 words):\n\
         Name: {}\n\
         Executable: {}\n\
         Memory: {} MB, CPU: {:.1}%\n\
         Reply with ONLY the description, nothing else.",
        name,
        exe_path.unwrap_or("unknown"),
        memory_mb,
        cpu_pct,
    )
}

/// Cache an AI-generated description for future lookups.
#[allow(dead_code)]
pub fn cache_ai_description(name: &str, description: &str) {
    if let Ok(mut cache) = AI_CACHE.lock() {
        cache.insert(name.to_string(), description.to_string());
    }
}

fn heuristic_classify(lower: &str, original: &str) -> (String, ProcessCategory, ResourceProfile) {
    // Helper suffixes
    if lower.ends_with(" helper") || lower.ends_with(" helper (renderer)") || lower.ends_with(" helper (gpu)") {
        let base = original.split(" Helper").next().unwrap_or(original);
        return (format!("{} subprocess", base), ProcessCategory::Unknown, ResourceProfile::Moderate);
    }

    // Compiler / build patterns
    if lower.contains("compile") || lower.contains("build") || lower.ends_with("c") && lower.len() <= 5 {
        return ("Compiler / build process".to_string(), ProcessCategory::DevTool, ResourceProfile::Heavy);
    }

    // Server patterns
    if lower.ends_with("-server") || lower.ends_with("d") && lower.len() > 3 || lower.ends_with("-daemon") {
        return (format!("{} service daemon", original), ProcessCategory::Server, ResourceProfile::Moderate);
    }

    // Worker patterns
    if lower.contains("worker") || lower.contains("agent") || lower.contains("helper") {
        return (format!("{} background worker", original), ProcessCategory::System, ResourceProfile::Light);
    }

    // Python/node script patterns
    if lower.starts_with("python") || lower.starts_with("node") {
        return ("Scripting runtime process".to_string(), ProcessCategory::DevTool, ResourceProfile::Bursty);
    }

    // Web/network
    if lower.contains("http") || lower.contains("proxy") || lower.contains("web") {
        return ("Web/network service".to_string(), ProcessCategory::Network, ResourceProfile::Moderate);
    }

    // Container
    if lower.contains("docker") || lower.contains("container") || lower.contains("kube") || lower.contains("pod") {
        return ("Container/orchestration service".to_string(), ProcessCategory::Container, ResourceProfile::Heavy);
    }

    // GPU/AI
    if lower.contains("cuda") || lower.contains("gpu") || lower.contains("tensor") || lower.contains("torch") || lower.contains("llm") || lower.contains("llama") {
        return ("AI/GPU compute process".to_string(), ProcessCategory::AI, ResourceProfile::Heavy);
    }

    // Default
    (format!("{} (unrecognized)", original), ProcessCategory::Unknown, ResourceProfile::Moderate)
}
