use std::collections::HashMap;

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
    pub timescale: String,
    pub signals: Vec<Signal>,
    pub changes: HashMap<String, Vec<ValueChange>>,
    pub max_time: u64,
}

impl VcdData {
    pub fn get_value_at(&self, id: &str, time: u64) -> String {
        let Some(changes) = self.changes.get(id) else { return "x".into() };
        if changes.is_empty() { return "x".into(); }
        // Binary search: last entry with time <= `time`
        let idx = changes.partition_point(|vc| vc.time <= time);
        if idx == 0 { return "x".into(); } // all changes after `time`
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
                    if part == "$end" { break; }
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
                    if part == "$end" { break; }
                }
            }
            "$upscope" => {
                scope_stack.pop();
                for part in tokens.by_ref() {
                    if part == "$end" { break; }
                }
            }
            "$var" => {
                let _type = if tokens.next().is_some() { () } else { continue };
                let width = tokens.next()
                    .and_then(|w| w.parse().ok())
                    .unwrap_or(1usize);
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
                    if part == "$end" { break; }
                }
                let scope = scope_stack.join(".");
                let full_name = if scope.is_empty() { name.clone() }
                    else { format!("{}.{}", scope, name) };
                signals.push(Signal { id: id.clone(), name, scope, full_name, width });
                changes.insert(id, Vec::new());
            }
            "$dumpvars" | "$dumpon" | "$dumpoff" | "$dumpall" => {
                loop {
                    let Some(t) = tokens.next() else { break };
                    if t == "$end" { break; }
                    if let Some((val, id)) = parse_val(t, &mut tokens) {
                        if let Some(v) = changes.get_mut(&id) { v.push(ValueChange { time: current_time, value: val }); }
                    }
                }
            }
            "$comment" | "$version" | "$date" => {
                for part in tokens.by_ref() {
                    if part == "$end" { break; }
                }
            }
            "$end" => {}
            t if t.starts_with('#') => {
                current_time = t[1..].parse().unwrap_or(0);
                if current_time > max_time { max_time = current_time; }
            }
            t => {
                if let Some((val, id)) = parse_val(t, &mut tokens) {
                    if let Some(v) = changes.get_mut(&id) { v.push(ValueChange { time: current_time, value: val }); }
                }
            }
        }
    }
    Ok(VcdData { timescale, signals, changes, max_time })
}

fn parse_val<'a>(tok: &'a str, tokens: &mut impl Iterator<Item = &'a str>) -> Option<(String, String)> {
    let first = tok.chars().next()?;
    match first {
        'b'|'B'|'r'|'R' => {
            let val = tok[1..].to_string();
            let id = tokens.next()?.to_string();
            Some((val, id))
        }
        '0'|'1'|'x'|'X'|'z'|'Z' if tok.len() >= 2 => {
            Some((first.to_lowercase().to_string(), tok[1..].to_string()))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sample_vcd() {
        let data = parse_vcd(SAMPLE_VCD).expect("sample VCD should parse");
        assert_eq!(data.timescale, "1 ns");
        assert_eq!(data.signals.len(), 7);
        assert_eq!(data.max_time, 200);
        assert_eq!(data.get_value_at("!", 0), "0");
        assert_eq!(data.get_value_at("#", 20), "10101010");
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
