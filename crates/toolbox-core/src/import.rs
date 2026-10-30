//! Read-only import of WinGuake's INI configuration.
//!
//! Adapted from sean2077/winguake's MIT-licensed INI importer, commit
//! eaffe0312b1f266a27f6878545389eac4737e4b2. Parsing produces a preview for
//! review before the host applies the imported configuration.

use std::collections::{HashMap, HashSet};

use crate::{AppProfile, Config, HotkeySpec, LaunchCommand, Platform};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportPreview {
    pub profiles: Vec<AppProfile>,
    pub warnings: Vec<String>,
}

/// Parse an original WinGuake INI into profiles ready for review before saving.
pub fn parse_ini(text: &str) -> Result<ImportPreview, String> {
    if text.len() > 1024 * 1024 || text.contains('\0') {
        return Err(
            "INI must be UTF-8 text under 1 MiB without NUL characters / INI 文件无效".into(),
        );
    }
    let mut sections: Vec<(String, HashMap<String, String>)> = Vec::new();
    let mut section_names = HashSet::new();
    let mut warnings = Vec::new();
    for (index, source) in text.trim_start_matches('\u{feff}').lines().enumerate() {
        let line = source.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if let Some(section) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            let section = section.trim();
            if section.is_empty() || section.contains(['[', ']']) {
                return Err(format!(
                    "Invalid INI section / INI 区块格式错误: {}",
                    index + 1
                ));
            }
            if !section_names.insert(section.to_ascii_lowercase()) {
                return Err(format!("Duplicate INI section / 重复区块: {section}"));
            }
            sections.push((section.to_owned(), HashMap::new()));
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("Invalid INI line / INI 格式错误: {}", index + 1));
        };
        let Some((section, values)) = sections.last_mut() else {
            return Err(format!(
                "INI key outside a section / 配置项不在区块内: {}",
                index + 1
            ));
        };
        let key = key.trim().to_ascii_lowercase();
        if key.is_empty() {
            return Err(format!("Empty INI key / 配置项名称为空: {}", index + 1));
        }
        if values
            .insert(key.clone(), value.trim().to_owned())
            .is_some()
        {
            warnings.push(format!("{section}: 重复配置项 {key}，采用最后一个值。"));
        }
    }

    if sections.is_empty() {
        return Err("No applications found / 未找到应用配置".into());
    }
    if sections.len() > 100 {
        return Err(
            "At most 100 application profiles are supported / 最多支持 100 个应用配置".into(),
        );
    }
    let mut ids = HashSet::new();
    let mut shortcuts = HashSet::new();
    let mut profiles = Vec::with_capacity(sections.len());
    for (section, values) in sections {
        let get = |key: &str| values.get(key).map(String::as_str).unwrap_or_default();
        let name = if get("name").is_empty() {
            section.as_str()
        } else {
            get("name")
        };
        let matcher = get("exe");
        let mut unknown = values
            .keys()
            .filter(|key| {
                !matches!(
                    key.as_str(),
                    "name"
                        | "exe"
                        | "hotkey"
                        | "launchcmd"
                        | "launchpaths"
                        | "disable"
                        | "maximize"
                        | "cyclecontinuous"
                )
            })
            .collect::<Vec<_>>();
        unknown.sort();
        for key in unknown {
            warnings.push(format!("{name}: 未识别的配置项 {key} 已忽略。"));
        }

        let base = section
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .take(40)
            .collect::<String>();
        let base = base.trim_matches('-');
        let base = if base.is_empty() { "imported" } else { base };
        let mut id = base.to_owned();
        let mut suffix = 2;
        while !ids.insert(id.clone()) {
            id = format!("{base}-{suffix}");
            suffix += 1;
        }

        let paths = get("launchpaths")
            .split('|')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        let primary = if get("launchcmd").is_empty() {
            None
        } else {
            Some(as_command(get("launchcmd"), &mut warnings, name)?)
        };
        let mut launch = paths
            .iter()
            .map(|path| as_command(path, &mut warnings, name))
            .collect::<Result<Vec<_>, _>>()?;
        if let Some(primary) = primary {
            for candidate in &mut launch {
                if candidate.args.is_empty() {
                    candidate.args.clone_from(&primary.args);
                }
            }
            if !launch.contains(&primary) {
                launch.insert(0, primary);
            }
            if !paths.is_empty() {
                warnings.push(format!(
                    "{name}: 已将 launchCmd 纳入候选；请确认命令与路径的优先级。"
                ));
            }
        }
        if matcher.is_empty() || launch.is_empty() {
            return Err(format!(
                "{name}: missing exe or launch command / 缺少进程名或启动方式"
            ));
        }

        let hotkey = if get("hotkey").is_empty() {
            String::new()
        } else {
            match convert_hotkey(get("hotkey")) {
                Ok(key) => key,
                Err(_) => {
                    warnings.push(format!(
                        "{name}: 无法转换原快捷键 {}，已留空，请手动设置。",
                        get("hotkey")
                    ));
                    String::new()
                }
            }
        };
        let (matcher, matcher_supported) = import_matcher(matcher, name, &mut warnings);
        let enabled = !parse_bool(get("disable"), false, name)? && matcher_supported;
        if enabled && !hotkey.is_empty() && !shortcuts.insert(hotkey.clone()) {
            warnings.push(format!(
                "{name}: 快捷键 {hotkey} 与其他导入项重复，保存前请修改。"
            ));
        }
        profiles.push(AppProfile {
            id,
            name: name.to_owned(),
            platform: Platform::Windows,
            matcher,
            hotkey,
            enabled,
            maximize: parse_bool(get("maximize"), false, name)?,
            cycle_continuous: parse_bool(get("cyclecontinuous"), true, name)?,
            quake: Default::default(),
            launch,
        });
    }

    // Validate structural bounds independently of the running OS and current
    // launcher shortcut. The UI validates the merged config before committing it.
    Config {
        profiles: profiles.clone(),
        windows_enabled: false,
        ..Config::default()
    }
    .validate()?;
    Ok(ImportPreview { profiles, warnings })
}

fn parse_bool(value: &str, fallback: bool, label: &str) -> Result<bool, String> {
    match value.to_ascii_lowercase().as_str() {
        "" => Ok(fallback),
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => Err(format!("{label}: invalid boolean / 布尔值无效: {value}")),
    }
}

fn import_matcher(value: &str, label: &str, warnings: &mut Vec<String>) -> (String, bool) {
    let lower = value.to_ascii_lowercase();
    let normalized = if lower.starts_with("ahk_exe ") {
        let executable = value[8..].trim();
        warnings.push(format!("{label}: 已将 ahk_exe 转换为精确进程名匹配。"));
        executable
    } else {
        value
    };
    let lower = normalized.to_ascii_lowercase();
    if normalized.is_empty()
        || lower.contains("ahk_")
        || lower.starts_with("title:")
        || lower.starts_with("regex:")
        || normalized.contains(['*', '?'])
    {
        warnings.push(format!("{label}: 窗口规则 {value} 无法转换为精确进程名；此项已禁用，请改为实际进程名（例如 Code.exe）。"));
        return (value.to_owned(), false);
    }
    (normalized.to_owned(), true)
}

fn convert_hotkey(value: &str) -> Result<String, String> {
    if !value.starts_with(['^', '!', '+', '#']) {
        return HotkeySpec::parse(value).map(|key| key.canonical());
    }
    let mut modifiers = Vec::new();
    let mut end = 0;
    for (index, character) in value.char_indices() {
        modifiers.push(match character {
            '^' => "Ctrl",
            '!' => "Alt",
            '+' => "Shift",
            '#' => "Super",
            _ => break,
        });
        end = index + character.len_utf8();
    }
    let key = &value[end..];
    let key = match key.to_ascii_lowercase().as_str() {
        "esc" => "Escape",
        "return" => "Enter",
        _ => key,
    };
    modifiers.push(key);
    HotkeySpec::parse(&modifiers.join("+")).map(|key| key.canonical())
}

/// Windows CRT-style argument splitting. Quotes and backslashes are data, not
/// shell syntax; an empty quoted argument is preserved.
pub fn split_windows_command_line(command: &str) -> Result<Vec<String>, String> {
    let characters = command.chars().collect::<Vec<_>>();
    let (mut index, mut quoted, mut started) = (0, false, false);
    let mut result = Vec::new();
    let mut word = String::new();
    while index < characters.len() {
        let character = characters[index];
        if character.is_whitespace() && !quoted {
            if started {
                result.push(std::mem::take(&mut word));
                started = false;
            }
            index += 1;
            continue;
        }
        if character == '\\' {
            let start = index;
            while index < characters.len() && characters[index] == '\\' {
                index += 1;
            }
            let count = index - start;
            if characters.get(index) == Some(&'"') {
                word.extend(std::iter::repeat_n('\\', count / 2));
                if count % 2 == 1 {
                    word.push('"');
                } else {
                    quoted = !quoted;
                }
                index += 1;
            } else {
                word.extend(std::iter::repeat_n('\\', count));
            }
            started = true;
            continue;
        }
        if character == '"' {
            quoted = !quoted;
        } else {
            word.push(character);
        }
        started = true;
        index += 1;
    }
    if quoted {
        return Err("Unclosed quotation mark / 启动命令引号未闭合".into());
    }
    if started {
        result.push(word);
    }
    Ok(result)
}

fn as_command(raw: &str, warnings: &mut Vec<String>, label: &str) -> Result<LaunchCommand, String> {
    // Original launchPaths entries often contained unquoted EXE paths with spaces.
    let is_plain_exe = !raw.contains(['"', '\r', '\n'])
        && raw.to_ascii_lowercase().ends_with(".exe")
        && raw.contains(['\\', '/']);
    let parts = if is_plain_exe {
        vec![raw.trim().to_owned()]
    } else {
        split_windows_command_line(raw)?
    };
    let Some(executable) = parts.first().filter(|executable| !executable.is_empty()) else {
        return Err(format!("{label}: empty launch command / 启动命令为空"));
    };
    let basename = executable
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(executable)
        .to_ascii_lowercase();
    if raw.contains(['&', '|', '<', '>'])
        || matches!(
            basename.as_str(),
            "start" | "cmd" | "cmd.exe" | "powershell" | "powershell.exe" | "pwsh" | "pwsh.exe"
        )
    {
        warnings.push(format!(
            "{label}: 请核对包含 shell 语法的命令；导入只保存配置，不执行命令。"
        ));
    }
    if basename == "code"
        || [".bat", ".cmd", ".ps1", ".vbs", ".js"]
            .iter()
            .any(|suffix| basename.ends_with(suffix))
    {
        warnings.push(format!(
            "{label}: 请将脚本启动项改成实际 EXE（例如 Code.exe）；启动器不会隐式调用脚本 shell。"
        ));
    }
    if raw.contains("A_") {
        warnings.push(format!(
            "{label}: 请将旧配置中的 A_ 变量和表达式替换成实际路径或受支持的环境变量。"
        ));
    }
    Ok(LaunchCommand {
        executable: executable.clone(),
        args: parts[1..].to_vec(),
        cwd: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_original_winguake_example() {
        let preview = parse_ini("[Notepad]\nhotkey=F7\nexe=notepad.exe\nlaunchCmd=notepad\nname=记事本\nlaunchPaths=notepad.exe|C:\\Windows\\System32\\notepad.exe\n; comment\ndisable=true\n").unwrap();
        let profile = &preview.profiles[0];
        assert_eq!(profile.name, "记事本");
        assert_eq!(profile.hotkey, "F7");
        assert!(!profile.enabled);
        assert_eq!(profile.platform, Platform::Windows);
        assert_eq!(profile.launch.len(), 3);
        assert!(profile.cycle_continuous);
    }

    #[test]
    fn preserves_windows_quoting_empty_arguments_and_slashes() {
        assert_eq!(
            split_windows_command_line(
                r#"program.exe "" "two words" C:\plain\path "quoted\"text""#
            )
            .unwrap(),
            vec![
                "program.exe",
                "",
                "two words",
                r"C:\plain\path",
                "quoted\"text"
            ]
        );
        assert_eq!(
            split_windows_command_line(r#"program.exe "C:\folder\\""#).unwrap(),
            vec!["program.exe", "C:\\folder\\"]
        );
        assert!(
            split_windows_command_line("app.exe \"unfinished")
                .unwrap_err()
                .contains("引号未闭合")
        );
    }

    #[test]
    fn inherits_explicit_arguments_and_keeps_command_priority() {
        let preview = parse_ini(
            r#"[Terminal]
exe=wezterm-gui.exe
launchCmd="C:\Program Files\WezTerm\wezterm-gui.exe" start --cwd "D:\work folder"
launchPaths=C:\Program Files\WezTerm\wezterm-gui.exe|D:\Portable Apps\WezTerm\wezterm-gui.exe
hotkey=^!t
maximize=true
cycleContinuous=false
"#,
        )
        .unwrap();
        let profile = &preview.profiles[0];
        assert_eq!(profile.launch.len(), 2);
        assert_eq!(
            profile.launch[0].executable,
            r"C:\Program Files\WezTerm\wezterm-gui.exe"
        );
        assert_eq!(
            profile.launch[1].args,
            vec!["start", "--cwd", r"D:\work folder"]
        );
        assert_eq!(profile.hotkey, "Ctrl+Alt+T");
        assert!(profile.maximize);
        assert!(!profile.cycle_continuous);
    }

    #[test]
    fn converts_only_equivalent_ahk_shortcuts() {
        for (input, expected) in [
            ("^!+a", "Ctrl+Alt+Shift+A"),
            ("+F3", "Shift+F3"),
            ("#F3", "Super+F3"),
            ("!+Tab", "Alt+Shift+TAB"),
            ("^Esc", "Ctrl+ESCAPE"),
            ("!Return", "Alt+ENTER"),
            ("Ctrl+Alt+Space", "Ctrl+Alt+SPACE"),
            ("F24", "F24"),
        ] {
            assert_eq!(convert_hotkey(input).unwrap(), expected);
        }
        for input in [
            "~F3", "*F3", "$F3", "<^a", "F3 up", "a & b", "vk07", "F25", "a",
        ] {
            assert!(convert_hotkey(input).is_err(), "{input}");
        }
        let preview = parse_ini("[App]\nexe=app.exe\nlaunchCmd=app.exe\nhotkey=~F3").unwrap();
        assert!(preview.profiles[0].hotkey.is_empty());
        assert!(
            preview
                .warnings
                .iter()
                .any(|w| w.contains("无法转换原快捷键"))
        );
    }

    #[test]
    fn accepts_bom_crlf_and_embedded_equals() {
        let preview = parse_ini("\u{feff}; comment\r\n[Browser]\r\nEXE=chrome.exe\r\nlaunchCmd=chrome.exe --app=https://example.com/?q=value\r\ndisable=off\r\n").unwrap();
        assert!(preview.profiles[0].enabled);
        assert_eq!(
            preview.profiles[0].launch[0].args,
            ["--app=https://example.com/?q=value"]
        );
    }

    #[test]
    fn rejects_bad_sections_and_boolean_values() {
        assert!(
            parse_ini("[App]\nexe=a.exe\nlaunchCmd=a.exe\n[app]\nexe=b.exe\nlaunchCmd=b.exe")
                .unwrap_err()
                .contains("重复区块")
        );
        assert!(
            parse_ini("[App]\nexe=app.exe\nlaunchCmd=app.exe\ndisable=maybe")
                .unwrap_err()
                .contains("invalid boolean")
        );
        assert!(parse_ini("exe=app.exe").is_err());
        assert!(parse_ini("[App]\nlaunchCmd=app.exe").is_err());
        assert!(parse_ini("[App]\nexe=app.exe").is_err());
    }

    #[test]
    fn creates_deterministic_unique_ids_for_unicode_names() {
        let ini = "[终端]\nexe=a.exe\nlaunchCmd=a.exe\n[浏览器]\nexe=b.exe\nlaunchCmd=b.exe\n[A B]\nexe=c.exe\nlaunchCmd=c.exe\n[A-B]\nexe=d.exe\nlaunchCmd=d.exe";
        let first = parse_ini(ini).unwrap();
        assert_eq!(first, parse_ini(ini).unwrap());
        assert_eq!(
            first
                .profiles
                .iter()
                .map(|p| p.id.as_str())
                .collect::<Vec<_>>(),
            ["imported", "imported-2", "a-b", "a-b-2"]
        );
    }

    #[test]
    fn keeps_shell_text_as_data_and_warns() {
        let preview =
            parse_ini("[Shell]\nexe=cmd.exe\nlaunchCmd=cmd.exe /c echo safe & exit\n").unwrap();
        assert_eq!(
            preview.profiles[0].launch[0].args,
            ["/c", "echo", "safe", "&", "exit"]
        );
        assert!(preview.warnings.iter().any(|w| w.contains("shell")));
    }

    #[test]
    fn warns_about_duplicate_keys_hotkeys_and_unsupported_fields() {
        let preview = parse_ini("[One]\nexe=a.exe\nlaunchCmd=a.exe\nhotkey=F3\nname=first\nname=last\nunknown=1\n[Two]\nexe=b.exe\nlaunchCmd=b.exe\nhotkey=F3").unwrap();
        assert_eq!(preview.profiles[0].name, "last");
        assert!(preview.warnings.iter().any(|w| w.contains("重复配置项")));
        assert!(preview.warnings.iter().any(|w| w.contains("未识别")));
        assert!(preview.warnings.iter().any(|w| w.contains("快捷键 F3")));
    }

    #[test]
    fn converts_process_matcher_and_disables_unsupported_ahk_window_rules() {
        let valid = parse_ini("[Code]\nexe=ahk_exe Code.exe\nlaunchCmd=Code.exe").unwrap();
        assert_eq!(valid.profiles[0].matcher, "Code.exe");
        assert!(valid.profiles[0].enabled);
        for matcher in [
            "ahk_class CabinetWClass",
            "ahk_exe Code.exe ahk_class Chrome_WidgetWin_1",
            "title:Editor",
            "*.exe",
        ] {
            let preview = parse_ini(&format!("[App]\nexe={matcher}\nlaunchCmd=app.exe")).unwrap();
            assert!(!preview.profiles[0].enabled);
            assert!(preview.warnings.iter().any(|w| w.contains("此项已禁用")));
        }
    }
}
