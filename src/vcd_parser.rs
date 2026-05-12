use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

#[derive(Debug, Clone)]
pub struct Signal {
    pub id: String,
    pub name: String,
    pub scope: String,
    pub full_name: String,
    pub width: usize,
}

#[derive(Debug, Clone)]
pub struct ValueChange {
    pub time: u64,
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct VcdData {
    pub format: String,
    pub timescale: String,
    pub signals: Vec<Signal>,
    pub changes: HashMap<String, Vec<ValueChange>>,
    pub max_time: u64,
}

impl VcdData {
    pub fn get_value_at(&self, id: &str, time: u64) -> String {
        let Some(changes) = self.changes.get(id) else {
            return "x".into();
        };
        if changes.is_empty() {
            return "x".into();
        }
        // Binary search: last entry with time <= `time`
        let idx = changes.partition_point(|vc| vc.time <= time);
        if idx == 0 {
            return "x".into();
        } // all changes after `time`
        changes[idx - 1].value.clone()
    }
}

pub fn parse_vcd(text: &str) -> Result<VcdData, String> {
    let mut tokens = text.split_whitespace();
    let mut timescale = "1 ns".to_string();
    let mut signals: Vec<Signal> = Vec::new();
    let mut changes: HashMap<String, Vec<ValueChange>> = HashMap::new();
    let mut max_time = 0u64;
    let mut current_time = 0u64;
    let mut scope_stack: Vec<String> = Vec::new();

    while let Some(tok) = tokens.next() {
        match tok {
            "$timescale" => {
                let mut parts = Vec::new();
                for part in tokens.by_ref() {
                    if part == "$end" {
                        break;
                    }
                    parts.push(part);
                }
                timescale = parts.join(" ");
            }
            "$scope" => {
                let _scope_type = tokens.next();
                if let Some(name) = tokens.next() {
                    scope_stack.push(name.to_string());
                }
                for part in tokens.by_ref() {
                    if part == "$end" {
                        break;
                    }
                }
            }
            "$upscope" => {
                scope_stack.pop();
                for part in tokens.by_ref() {
                    if part == "$end" {
                        break;
                    }
                }
            }
            "$var" => {
                let _type = if tokens.next().is_some() {
                    ()
                } else {
                    continue;
                };
                let width = tokens.next().and_then(|w| w.parse().ok()).unwrap_or(1usize);
                let id = if let Some(id) = tokens.next() {
                    id.to_string()
                } else {
                    continue;
                };
                let name = if let Some(name) = tokens.next() {
                    name.to_string()
                } else {
                    continue;
                };
                for part in tokens.by_ref() {
                    if part == "$end" {
                        break;
                    }
                }
                let scope = scope_stack.join(".");
                let full_name = if scope.is_empty() {
                    name.clone()
                } else {
                    format!("{}.{}", scope, name)
                };
                signals.push(Signal {
                    id: id.clone(),
                    name,
                    scope,
                    full_name,
                    width,
                });
                changes.insert(id, Vec::new());
            }
            "$dumpvars" | "$dumpon" | "$dumpoff" | "$dumpall" => loop {
                let Some(t) = tokens.next() else { break };
                if t == "$end" {
                    break;
                }
                if let Some((val, id)) = parse_val(t, &mut tokens) {
                    if let Some(v) = changes.get_mut(&id) {
                        v.push(ValueChange {
                            time: current_time,
                            value: val,
                        });
                    }
                }
            },
            "$comment" | "$version" | "$date" => {
                for part in tokens.by_ref() {
                    if part == "$end" {
                        break;
                    }
                }
            }
            "$end" => {}
            t if t.starts_with('#') => {
                current_time = t[1..].parse().unwrap_or(0);
                if current_time > max_time {
                    max_time = current_time;
                }
            }
            t => {
                if let Some((val, id)) = parse_val(t, &mut tokens) {
                    if let Some(v) = changes.get_mut(&id) {
                        v.push(ValueChange {
                            time: current_time,
                            value: val,
                        });
                    }
                }
            }
        }
    }
    Ok(VcdData {
        format: "VCD".into(),
        timescale,
        signals,
        changes,
        max_time,
    })
}

fn parse_val<'a>(
    tok: &'a str,
    tokens: &mut impl Iterator<Item = &'a str>,
) -> Option<(String, String)> {
    let first = tok.chars().next()?;
    match first {
        'b' | 'B' | 'r' | 'R' => {
            let val = tok[1..].to_string();
            let id = tokens.next()?.to_string();
            Some((val, id))
        }
        '0' | '1' | 'x' | 'X' | 'z' | 'Z' if tok.len() >= 2 => {
            Some((first.to_lowercase().to_string(), tok[1..].to_string()))
        }
        _ => None,
    }
}

pub fn parse_trace(text: &str) -> Result<VcdData, String> {
    if text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .is_some_and(|line| line.starts_with("@xtrace"))
    {
        parse_xtrace(text)
    } else {
        parse_vcd(text)
    }
}

pub fn parse_trace_file(path: &str) -> Result<VcdData, String> {
    if file_starts_with_xtrace(path)? {
        parse_xtrace_file_partial(path)
    } else {
        let mut text = String::new();
        File::open(path)
            .map_err(|e| e.to_string())?
            .read_to_string(&mut text)
            .map_err(|e| e.to_string())?;
        parse_vcd(&text)
    }
}

pub fn load_xtrace_signal_changes(
    path: &str,
    targets: &[(String, usize)],
) -> Result<HashMap<String, Vec<ValueChange>>, String> {
    let target_widths: HashMap<&str, usize> = targets
        .iter()
        .map(|(id, width)| (id.as_str(), *width))
        .collect();
    let mut changes: HashMap<String, Vec<ValueChange>> = targets
        .iter()
        .map(|(id, _)| (id.clone(), Vec::new()))
        .collect();
    let file = File::open(path).map_err(|e| e.to_string())?;
    let reader = BufReader::with_capacity(1024 * 1024, file);
    let mut section = String::new();
    let mut current_time = 0u64;

    for (line_no, raw_line) in reader.lines().enumerate() {
        let raw_line = raw_line.map_err(|e| e.to_string())?;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('@') {
            if let Some(next_section) = parse_section_directive(line) {
                section = next_section;
            }
            continue;
        }
        if section == "dict" {
            continue;
        }
        if let Some(delta) = parse_xtrace_time_delta(line) {
            current_time = current_time.saturating_add(delta);
            continue;
        }

        let fields =
            split_xtrace_fields(line).map_err(|e| format!("line {}: {}", line_no + 1, e))?;
        let Some(kind) = fields.first().map(String::as_str) else {
            continue;
        };
        match kind {
            "D" if fields.len() >= 3 => {
                if let Some(width) = target_widths.get(fields[1].as_str()) {
                    let value = normalize_xtrace_value(&fields[2], *width);
                    if let Some(slot) = changes.get_mut(&fields[1]) {
                        slot.push(ValueChange {
                            time: current_time,
                            value,
                        });
                    }
                }
            }
            "P" => {
                for assign in &fields[1..] {
                    let Some((id, value)) = assign.split_once('=') else {
                        continue;
                    };
                    if let Some(width) = target_widths.get(id) {
                        if let Some(slot) = changes.get_mut(id) {
                            slot.push(ValueChange {
                                time: current_time,
                                value: normalize_xtrace_value(value, *width),
                            });
                        }
                    }
                }
            }
            "N" => {
                for assign in &fields[2..] {
                    let Some((id, value)) = assign.split_once('=') else {
                        continue;
                    };
                    if let Some(width) = target_widths.get(id) {
                        if let Some(slot) = changes.get_mut(id) {
                            slot.push(ValueChange {
                                time: current_time,
                                value: normalize_xtrace_value(value, *width),
                            });
                        }
                    }
                }
            }
            _ => {}
        }
    }

    Ok(changes)
}

fn file_starts_with_xtrace(path: &str) -> Result<bool, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let reader = BufReader::new(file);
    for raw_line in reader.lines().take(32) {
        let raw_line = raw_line.map_err(|e| e.to_string())?;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        return Ok(line.starts_with("@xtrace"));
    }
    Ok(Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "xtrace" | "xtr" | "xt" | "xtt"
            )
        })
        .unwrap_or(false))
}

fn parse_xtrace_file_partial(path: &str) -> Result<VcdData, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let reader = BufReader::with_capacity(1024 * 1024, file);
    let mut saw_header = false;
    let mut timescale = "1ns".to_string();
    let mut section = String::new();
    let mut modules: HashMap<String, String> = HashMap::new();
    let mut signals: Vec<Signal> = Vec::new();
    let mut seen_signals: HashSet<String> = HashSet::new();
    let mut current_time = 0u64;
    let mut max_time = 0u64;

    for (line_no, raw_line) in reader.lines().enumerate() {
        let raw_line = raw_line.map_err(|e| e.to_string())?;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('@') {
            let mut parts = line.split_whitespace();
            match parts.next() {
                Some("@xtrace") => saw_header = true,
                Some("@timescale") => {
                    if let Some(ts) = parts.next() {
                        timescale = ts.to_string();
                    }
                }
                Some("@section") => {
                    section = parts.next().unwrap_or_default().to_string();
                }
                _ => {}
            }
            continue;
        }
        if section == "dict" {
            parse_xtrace_dict_line(
                line,
                line_no + 1,
                &mut modules,
                &mut signals,
                &mut seen_signals,
            )?;
            continue;
        }
        if let Some(delta) = parse_xtrace_time_delta(line) {
            current_time = current_time.saturating_add(delta);
            max_time = max_time.max(current_time);
        }
    }

    if !saw_header {
        return Err("missing @xtrace header".into());
    }

    let changes = signals
        .iter()
        .map(|sig| (sig.id.clone(), Vec::new()))
        .collect();
    Ok(VcdData {
        format: "XTrace".into(),
        timescale,
        signals,
        changes,
        max_time,
    })
}

pub fn parse_xtrace(text: &str) -> Result<VcdData, String> {
    let mut saw_header = false;
    let mut timescale = "1ns".to_string();
    let mut section = String::new();
    let mut modules: HashMap<String, String> = HashMap::new();
    let mut signals: Vec<Signal> = Vec::new();
    let mut signal_widths: HashMap<String, usize> = HashMap::new();
    let mut changes: HashMap<String, Vec<ValueChange>> = HashMap::new();
    let mut current_time = 0u64;
    let mut max_time = 0u64;

    for (line_no, raw_line) in text.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('@') {
            let mut parts = line.split_whitespace();
            match parts.next() {
                Some("@xtrace") => saw_header = true,
                Some("@timescale") => {
                    if let Some(ts) = parts.next() {
                        timescale = ts.to_string();
                    }
                }
                Some("@section") => {
                    section = parts.next().unwrap_or_default().to_string();
                }
                _ => {}
            }
            continue;
        }

        let fields =
            split_xtrace_fields(line).map_err(|e| format!("line {}: {}", line_no + 1, e))?;
        let Some(kind) = fields.first().map(String::as_str) else {
            continue;
        };

        match (section.as_str(), kind) {
            ("dict", "M") => {
                if fields.len() < 3 {
                    return Err(format!("line {}: malformed module record", line_no + 1));
                }
                modules.insert(fields[1].clone(), normalize_xtrace_scope(&fields[2]));
            }
            ("dict", "S") => {
                if fields.len() < 5 {
                    return Err(format!("line {}: malformed signal record", line_no + 1));
                }
                let id = fields[1].clone();
                let module_id = &fields[2];
                let name = fields[3].clone();
                let ty = fields[4].as_str();
                let attrs = parse_attrs(&fields[5..]);
                let scope = modules.get(module_id).cloned().unwrap_or_default();
                let width = attrs
                    .get("width")
                    .and_then(|w| w.parse::<usize>().ok())
                    .unwrap_or_else(|| xtrace_type_width(ty));
                let full_name = if scope.is_empty() {
                    name.clone()
                } else {
                    format!("{}.{}", scope, name)
                };
                signals.push(Signal {
                    id: id.clone(),
                    name,
                    scope,
                    full_name,
                    width,
                });
                signal_widths.insert(id.clone(), width);
                changes.entry(id).or_default();
            }
            (_, "T") if fields.len() >= 2 => {
                let delta = fields[1]
                    .strip_prefix('+')
                    .unwrap_or(&fields[1])
                    .parse::<u64>()
                    .map_err(|_| format!("line {}: invalid time delta", line_no + 1))?;
                current_time = current_time.saturating_add(delta);
                max_time = max_time.max(current_time);
            }
            (_, "D") => {
                if fields.len() < 3 {
                    return Err(format!("line {}: malformed delta record", line_no + 1));
                }
                push_xtrace_change(
                    &mut changes,
                    &signal_widths,
                    &fields[1],
                    &fields[2],
                    current_time,
                );
            }
            (_, "P") => {
                for assign in &fields[1..] {
                    if let Some((id, value)) = assign.split_once('=') {
                        push_xtrace_change(&mut changes, &signal_widths, id, value, current_time);
                    }
                }
            }
            (_, "N") => {
                for assign in &fields[2..] {
                    if let Some((id, value)) = assign.split_once('=') {
                        push_xtrace_change(&mut changes, &signal_widths, id, value, current_time);
                    }
                }
            }
            _ => {}
        }
    }

    if !saw_header {
        return Err("missing @xtrace header".into());
    }

    Ok(VcdData {
        format: "XTrace".into(),
        timescale,
        signals,
        changes,
        max_time,
    })
}

fn split_xtrace_fields(line: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = line.chars();
    let mut quoted = false;
    while let Some(ch) = chars.next() {
        match ch {
            '"' => {
                quoted = !quoted;
                cur.push(ch);
            }
            '\\' if quoted => {
                cur.push(ch);
                if let Some(next) = chars.next() {
                    cur.push(next);
                }
            }
            ',' if !quoted => {
                out.push(cur.trim().to_string());
                cur.clear();
            }
            _ => cur.push(ch),
        }
    }
    if quoted {
        return Err("unterminated quoted string".into());
    }
    out.push(cur.trim().to_string());
    Ok(out)
}

fn parse_attrs(fields: &[String]) -> HashMap<String, String> {
    fields
        .iter()
        .filter_map(|field| {
            let (k, v) = field.split_once('=')?;
            Some((k.to_string(), v.to_string()))
        })
        .collect()
}

fn parse_section_directive(line: &str) -> Option<String> {
    let mut parts = line.split_whitespace();
    (parts.next() == Some("@section")).then(|| parts.next().unwrap_or_default().to_string())
}

fn parse_xtrace_time_delta(line: &str) -> Option<u64> {
    let rest = line.strip_prefix("T,+")?;
    let end = rest.find(',').unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn parse_xtrace_dict_line(
    line: &str,
    line_no: usize,
    modules: &mut HashMap<String, String>,
    signals: &mut Vec<Signal>,
    seen_signals: &mut HashSet<String>,
) -> Result<(), String> {
    let fields = split_xtrace_fields(line).map_err(|e| format!("line {}: {}", line_no, e))?;
    match fields.first().map(String::as_str) {
        Some("M") => {
            if fields.len() < 3 {
                return Err(format!("line {}: malformed module record", line_no));
            }
            modules.insert(fields[1].clone(), normalize_xtrace_scope(&fields[2]));
        }
        Some("S") => {
            if fields.len() < 5 {
                return Err(format!("line {}: malformed signal record", line_no));
            }
            let id = fields[1].clone();
            if !seen_signals.insert(id.clone()) {
                return Ok(());
            }
            let module_id = &fields[2];
            let name = fields[3].clone();
            let ty = fields[4].as_str();
            let attrs = parse_attrs(&fields[5..]);
            let scope = modules.get(module_id).cloned().unwrap_or_default();
            let width = attrs
                .get("width")
                .and_then(|w| w.parse::<usize>().ok())
                .unwrap_or_else(|| xtrace_type_width(ty));
            let full_name = if scope.is_empty() {
                name.clone()
            } else {
                format!("{}.{}", scope, name)
            };
            signals.push(Signal {
                id,
                name,
                scope,
                full_name,
                width,
            });
        }
        _ => {}
    }
    Ok(())
}

fn normalize_xtrace_scope(path: &str) -> String {
    path.trim_matches('/').replace('/', ".")
}

fn xtrace_type_width(ty: &str) -> usize {
    match ty {
        "bit" => 1,
        "u8" | "s8" => 8,
        "u16" | "s16" => 16,
        "u32" | "s32" => 32,
        "u64" | "s64" => 64,
        _ if ty.starts_with("logic[") && ty.ends_with(']') => {
            ty[6..ty.len() - 1].parse::<usize>().unwrap_or(1)
        }
        _ => 1,
    }
}

fn push_xtrace_change(
    changes: &mut HashMap<String, Vec<ValueChange>>,
    signal_widths: &HashMap<String, usize>,
    id: &str,
    value: &str,
    time: u64,
) {
    let Some(width) = signal_widths.get(id).copied() else {
        return;
    };
    let value = normalize_xtrace_value(value, width);
    changes
        .entry(id.to_string())
        .or_default()
        .push(ValueChange { time, value });
}

fn normalize_xtrace_value(value: &str, width: usize) -> String {
    let raw = value.trim().trim_matches('"');
    if let Some(bin) = raw.strip_prefix("0b").or_else(|| raw.strip_prefix("0B")) {
        let bits: String = bin
            .chars()
            .map(|c| match c {
                '0' => '0',
                '1' => '1',
                'x' | 'X' => 'x',
                'z' | 'Z' => 'z',
                _ => 'x',
            })
            .collect();
        if width == 1 {
            return bits.chars().last().unwrap_or('x').to_string();
        }
        return fit_bits_pad(bits, width, 'x');
    }
    if let Some(hex) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
        if width == 1 {
            return hex
                .chars()
                .rev()
                .find_map(|ch| match ch {
                    'x' | 'X' => Some("x".to_string()),
                    'z' | 'Z' => Some("z".to_string()),
                    _ => ch.to_digit(16).map(|n| {
                        if n & 1 == 1 {
                            "1".to_string()
                        } else {
                            "0".to_string()
                        }
                    }),
                })
                .unwrap_or_else(|| "x".to_string());
        }
        let mut bits = String::with_capacity(hex.len() * 4);
        for ch in hex.chars() {
            match ch {
                'x' | 'X' => bits.push_str("xxxx"),
                'z' | 'Z' => bits.push_str("zzzz"),
                _ => match ch.to_digit(16) {
                    Some(n) => bits.push_str(&format!("{:04b}", n)),
                    None => return raw.to_string(),
                },
            }
        }
        return fit_bits_pad(bits, width, '0');
    }
    if width == 1 {
        return match raw.chars().next().unwrap_or('x') {
            '0' => "0".into(),
            '1' => "1".into(),
            'z' | 'Z' => "z".into(),
            'x' | 'X' => "x".into(),
            _ => "x".into(),
        };
    }
    if raw.eq_ignore_ascii_case("x") {
        return "x".repeat(width);
    }
    if raw.eq_ignore_ascii_case("z") {
        return "z".repeat(width);
    }
    if raw.chars().all(|c| matches!(c, '0' | '1' | 'x' | 'X' | 'z' | 'Z')) {
        let bits: String = raw
            .chars()
            .map(|c| match c {
                'X' => 'x',
                'Z' => 'z',
                other => other,
            })
            .collect();
        return fit_bits_pad(bits, width, '0');
    }
    raw.to_string()
}

fn fit_bits_pad(bits: String, width: usize, _pad: char) -> String {
    if bits.len() > width {
        bits[bits.len() - width..].to_string()
    } else if bits.len() < width {
        let pad_ch = match bits.chars().next() {
            Some('x') => 'x',
            Some('z') => 'z',
            _ => '0',
        };
        format!(
            "{}{}",
            std::iter::repeat(pad_ch)
                .take(width - bits.len())
                .collect::<String>(),
            bits
        )
    } else {
        bits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sample_vcd() {
        let data = parse_vcd(SAMPLE_VCD).expect("sample VCD should parse");
        assert_eq!(data.format, "VCD");
        assert_eq!(data.timescale, "1 ns");
        assert_eq!(data.signals.len(), 7);
        assert_eq!(data.max_time, 200);
        assert_eq!(data.get_value_at("!", 0), "0");
        assert_eq!(data.get_value_at("#", 20), "10101010");
    }

    #[test]
    fn parses_sample_xtrace() {
        let data = parse_trace(SAMPLE_XTRACE).expect("sample XTrace should parse");
        assert_eq!(data.format, "XTrace");
        assert_eq!(data.timescale, "1ns");
        assert_eq!(data.signals.len(), 3);
        assert_eq!(data.max_time, 2);
        assert_eq!(data.signals[0].full_name, "top.cpu.pc");
        assert_eq!(data.get_value_at("s0", 1), "0001001000110100");
        assert_eq!(data.get_value_at("s2", 2), "0");
    }

    #[test]
    fn normalizes_binary_prefixed_values() {
        assert_eq!(normalize_xtrace_value("0bX", 1), "x");
        assert_eq!(normalize_xtrace_value("0b1", 1), "1");
        assert_eq!(normalize_xtrace_value("0b0", 1), "0");
        assert_eq!(normalize_xtrace_value("0b00XX", 4), "00xx");
        assert_eq!(normalize_xtrace_value("0b1010", 4), "1010");
        assert_eq!(normalize_xtrace_value("0b0000XXXX1XXXXXXX", 16), "0000xxxx1xxxxxxx");
        assert_eq!(normalize_xtrace_value("0bXXXX", 4), "xxxx");
    }

    #[test]
    fn normalizes_hex_prefixed_values() {
        assert_eq!(normalize_xtrace_value("0x0", 1), "0");
        assert_eq!(normalize_xtrace_value("0x1", 1), "1");
        assert_eq!(normalize_xtrace_value("0xF", 4), "1111");
        assert_eq!(normalize_xtrace_value("0x82", 16), "0000000010000010");
        assert_eq!(normalize_xtrace_value("0xX", 1), "x");
    }
}

pub const SAMPLE_VCD: &str = r#"$timescale 1 ns $end
$scope module tb $end
$var wire 1 ! clk $end
$var wire 1 " rst $end
$var wire 8 # data $end
$var wire 1 $ valid $end
$var wire 1 % ready $end
$upscope $end
$scope module dut $end
$var wire 8 & out $end
$var wire 1 ' done $end
$upscope $end
$dumpvars
0!
1"
b00000000 #
0$
0%
b00000000 &
0'
$end
#10
1!
#20
0!
0"
b10101010 #
1$
1%
#30
1!
#40
0!
b11001100 #
#50
1!
b10101010 &
1'
#60
0!
b11110000 #
0$
#70
1!
#80
0!
b00001111 #
1$
#90
1!
b11001100 &
#100
0!
0'
b01010101 #
#110
1!
#120
0!
b11111111 #
0$
#130
1!
#140
0!
b01010101 &
1'
#150
1!
#160
0!
0'
b00000000 #
0$
#170
1!
#180
0!
b10000001 #
1$
#190
1!
#200
0!
"#;

pub const SAMPLE_XTRACE: &str = r#"@xtrace 1.0
@format text
@timescale 1ns
@section dict
M,m0,/top/cpu
S,s0,m0,pc,u16,tags=pc|state
S,s1,m0,state,enum:cpu_state,tags=fsm,width=2
S,s2,m0,valid,bit,tags=control
@section trace
T,+0
N,full,s0=0x0000,s1=0,s2=0
T,+1
P,s0=0x1234,s1=1,s2=1
T,+1
D,s2,0
@section end
"#;
