//! Private GCR1 snapshot carried across the selected-package commit boundary.
//! The file bytes are external operator input, never a verified package artifact.
use crate::engine_functions::contract::hash_bytes;
use serde_json::{json, Value};

pub(super) const MAX_PACKET: usize = 4 * 1024 * 1024 + 256 * 1024 + 128 * 1024;
// Selected shim JSONC parser allows a 1 MiB expanded tree plus lexer/callback temporaries.
pub(super) const MAX_PARSE_TEMP: usize = 2 * 1024 * 1024;
const MAX_RAW: usize = 4 * 1024 * 1024;
const MAX_METADATA: usize = 256 * 1024;

#[derive(Clone, Debug)]
pub(super) struct Effect {
    section: String,
    name: String,
    platform: String,
    result: String,
    validator: String,
    error: String,
}
#[derive(Clone, Debug)]
pub(super) struct Repair {
    path: String,
    pub(super) bytes: Vec<u8>,
    result: String,
    error: String,
    effects: Vec<Effect>,
}
#[derive(Clone, Debug)]
pub(super) struct Snapshot {
    pub(super) repairs: Vec<Repair>,
    pub(super) custom_paths: Vec<String>,
}
struct Reader<'a> {
    input: &'a [u8],
    at: usize,
    metadata: usize,
    raw: usize,
}
impl Reader<'_> {
    fn count(&mut self) -> Result<usize, String> {
        let end = self
            .at
            .checked_add(4)
            .ok_or("repair snapshot length overflow")?;
        let bytes: [u8; 4] = self
            .input
            .get(self.at..end)
            .ok_or("truncated repair snapshot")?
            .try_into()
            .unwrap();
        self.at = end;
        Ok(u32::from_le_bytes(bytes) as usize)
    }
    fn field(&mut self, max: usize, raw: bool) -> Result<&[u8], String> {
        let n = self.count()?;
        if n > max {
            return Err("repair snapshot field size limit".into());
        }
        let total = if raw {
            &mut self.raw
        } else {
            &mut self.metadata
        };
        *total = total
            .checked_add(n)
            .ok_or("repair snapshot length overflow")?;
        if *total > if raw { MAX_RAW } else { MAX_METADATA } {
            return Err("repair snapshot aggregate size limit".into());
        }
        let end = self
            .at
            .checked_add(n)
            .ok_or("repair snapshot length overflow")?;
        let value = self
            .input
            .get(self.at..end)
            .ok_or("truncated repair snapshot")?;
        self.at = end;
        Ok(value)
    }
    fn text(&mut self, max: usize) -> Result<String, String> {
        Ok(std::str::from_utf8(self.field(max, false)?)
            .map_err(|_| "invalid repair snapshot UTF-8")?
            .to_owned())
    }
}
fn safe_path(path: &str) -> bool {
    let Some(name) = path.strip_prefix("custom/") else {
        return false;
    };
    !name.is_empty()
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains(':')
        && !name.contains('\0')
        && name != "."
        && name != ".."
        && (name.ends_with(".json") || name.ends_with(".jsonc"))
}
pub(super) fn decode(input: &[u8]) -> Result<Snapshot, String> {
    if input.len() > MAX_PACKET || input.get(..4) != Some(b"GCR1") {
        return Err("invalid repair snapshot version or size".into());
    }
    let mut r = Reader {
        input,
        at: 4,
        metadata: 0,
        raw: 0,
    };
    let count = r.count()?;
    if count > 64 {
        return Err("repair snapshot file count limit".into());
    }
    let mut repairs = Vec::with_capacity(count);
    let mut previous = String::new();
    let mut effects_total = 0usize;
    for _ in 0..count {
        let path = r.text(240)?;
        if !safe_path(&path) || (!previous.is_empty() && path <= previous) {
            return Err(format!(
                "invalid, duplicate or unsorted repair snapshot path: {path}"
            ));
        }
        previous.clone_from(&path);
        let result = r.text(32)?;
        if ![
            "read-error",
            "parse-error",
            "empty",
            "applied",
            "type-error",
        ]
        .contains(&result.as_str())
        {
            return Err(format!("{path}: invalid repair result"));
        }
        let error = r.text(4096)?;
        let bytes = r.field(256 * 1024, true)?.to_vec();
        let count = r.count()?;
        effects_total = effects_total
            .checked_add(count)
            .ok_or("repair effect count overflow")?;
        if effects_total > 4096 {
            return Err(format!("{path}: repair effect count limit"));
        }
        let mut effects = Vec::with_capacity(count);
        for _ in 0..count {
            let section = r.text(32)?;
            let name = r.text(240)?;
            let platform = r.text(64)?;
            let effect_result = r.text(32)?;
            let validator = r.text(32)?;
            let effect_error = r.text(4096)?;
            if (![
                "interfaces",
                "offsets",
                "signatures",
                "keys",
                "calls",
                "hooks",
            ]
            .contains(&section.as_str())
                && section != "document"
                && effect_result != "ignored")
                || !["applied", "invalid", "other-platform", "ignored"]
                    .contains(&effect_result.as_str())
                || !["", "unchanged", "explicit", "empty", "carried", "disarmed"]
                    .contains(&validator.as_str())
                || ((section == "offsets" || section == "signatures") != !platform.is_empty())
                || (effect_result == "invalid") != !effect_error.is_empty()
                || (effect_result == "ignored" && (!name.is_empty() || !validator.is_empty()))
                || section.is_empty()
                || (section == "document" && effect_result != "invalid")
            {
                return Err(format!("{path}: invalid repair effect"));
            }
            effects.push(Effect {
                section,
                name,
                platform,
                result: effect_result,
                validator,
                error: effect_error,
            });
        }
        if (matches!(result.as_str(), "read-error" | "parse-error" | "type-error"))
            != !error.is_empty()
        {
            return Err(format!("{path}: repair result/error mismatch"));
        }
        if matches!(result.as_str(), "read-error" | "parse-error") && !effects.is_empty() {
            return Err(format!("{path}: unparsed repair has effects"));
        }
        if result == "type-error" && !effects.iter().any(|e| e.result == "invalid") {
            return Err(format!("{path}: missing invalid entry"));
        }
        repairs.push(Repair {
            path,
            bytes,
            result,
            error,
            effects,
        });
    }
    let paths_count = r.count()?;
    if paths_count > 64 {
        return Err("repair applied path count limit".into());
    }
    let mut custom_paths = Vec::with_capacity(paths_count);
    for _ in 0..paths_count {
        custom_paths.push(r.text(240)?);
    }
    let expected: Vec<&str> = repairs
        .iter()
        .filter(|item| !matches!(item.result.as_str(), "read-error" | "parse-error"))
        .map(|item| item.path.as_str())
        .collect();
    if expected != custom_paths.iter().map(String::as_str).collect::<Vec<_>>() {
        return Err("repair snapshot captured/applied identity mismatch".into());
    }
    if r.at != input.len() {
        return Err("repair snapshot trailing bytes".into());
    }
    Ok(Snapshot {
        repairs,
        custom_paths,
    })
}
impl Snapshot {
    pub(super) fn status(&self) -> Value {
        json!(self
            .repairs
            .iter()
            .map(|record| json!({
                "path": record.path, "sha256": hash_bytes(&record.bytes),
                "result": record.result, "error": record.error,
                "effects": record.effects.iter().map(|effect| json!({
                    "section": effect.section, "name": effect.name, "platform": effect.platform,
                    "result": effect.result, "validator": effect.validator, "error": effect.error,
                })).collect::<Vec<_>>()
            }))
            .collect::<Vec<_>>())
    }
    pub(super) fn retained_bytes(&self) -> usize {
        self.repairs.capacity() * std::mem::size_of::<Repair>()
            + self.custom_paths.capacity() * std::mem::size_of::<String>()
            + self
                .repairs
                .iter()
                .map(|record| {
                    record.bytes.capacity()
                        + record.path.capacity()
                        + record.result.capacity()
                        + record.error.capacity()
                        + record.effects.capacity() * std::mem::size_of::<Effect>()
                        + record
                            .effects
                            .iter()
                            .map(|effect| {
                                effect.section.capacity()
                                    + effect.name.capacity()
                                    + effect.platform.capacity()
                                    + effect.result.capacity()
                                    + effect.validator.capacity()
                                    + effect.error.capacity()
                            })
                            .sum::<usize>()
                })
                .sum::<usize>()
            + self
                .custom_paths
                .iter()
                .map(String::capacity)
                .sum::<usize>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn word(out: &mut Vec<u8>, n: usize) {
        out.extend_from_slice(&(n as u32).to_le_bytes());
    }
    fn field(out: &mut Vec<u8>, value: &[u8]) {
        word(out, value.len());
        out.extend_from_slice(value);
    }
    fn record(
        out: &mut Vec<u8>,
        path: &str,
        bytes: &[u8],
        result: &str,
        error: &str,
        effects: &[(&str, &str, &str, &str, &str, &str)],
    ) {
        field(out, path.as_bytes());
        field(out, result.as_bytes());
        field(out, error.as_bytes());
        field(out, bytes);
        word(out, effects.len());
        for tuple in effects {
            for value in [tuple.0, tuple.1, tuple.2, tuple.3, tuple.4, tuple.5] {
                field(out, value.as_bytes());
            }
        }
    }
    fn packet(records: &[(&str, &[u8], &str, &str)], paths: &[&str]) -> Vec<u8> {
        let mut out = b"GCR1".to_vec();
        word(&mut out, records.len());
        for &(path, bytes, result, error) in records {
            record(&mut out, path, bytes, result, error, &[]);
        }
        word(&mut out, paths.len());
        for path in paths {
            field(&mut out, path.as_bytes());
        }
        out
    }
    #[test]
    fn exact_external_bytes_and_failed_file_have_distinct_status() {
        let exact = b"// repair\n {\"signatures\":{}}\n";
        let mut out = b"GCR1".to_vec();
        word(&mut out, 2);
        record(
            &mut out,
            "custom/10.jsonc",
            exact,
            "applied",
            "",
            &[(
                "signatures",
                "S",
                "linuxsteamrt64",
                "applied",
                "carried",
                "",
            )],
        );
        record(
            &mut out,
            "custom/20.jsonc",
            b"{bad",
            "parse-error",
            "parse failed",
            &[],
        );
        word(&mut out, 1);
        field(&mut out, b"custom/10.jsonc");
        let snapshot = decode(&out).unwrap();
        assert_eq!(snapshot.repairs[0].bytes, exact);
        let status = snapshot.status();
        assert_eq!(status[0]["sha256"], hash_bytes(exact));
        assert_eq!(status[0]["effects"][0]["validator"], "carried");
        assert_eq!(status[1]["result"], "parse-error");
        assert_eq!(status[1]["sha256"], hash_bytes(b"{bad"));
        assert!(!status.to_string().contains("// repair"));
        assert!(!status.to_string().contains("{bad"));
    }
    #[test]
    fn rejects_count_total_identity_order_encoding_and_truncation() {
        let empty = packet(&[], &[]);
        assert!(decode(&empty).is_ok());
        let bad = packet(&[("custom/a.jsonc", b"{}", "applied", "")], &[]);
        assert!(decode(&bad).unwrap_err().contains("identity"));
        let duplicate = packet(
            &[
                ("custom/a.jsonc", b"{}", "empty", ""),
                ("custom/a.jsonc", b"{}", "empty", ""),
            ],
            &["custom/a.jsonc", "custom/a.jsonc"],
        );
        assert!(decode(&duplicate).unwrap_err().contains("unsorted"));
        let reversed = packet(
            &[
                ("custom/z.jsonc", b"{}", "empty", ""),
                ("custom/a.jsonc", b"{}", "empty", ""),
            ],
            &["custom/z.jsonc", "custom/a.jsonc"],
        );
        assert!(decode(&reversed).unwrap_err().contains("unsorted"));
        let wrong_path = packet(&[("../a.jsonc", b"{}", "empty", "")], &["../a.jsonc"]);
        assert!(decode(&wrong_path).unwrap_err().contains("path"));
        let mut invalid_utf8 = packet(
            &[("custom/a.jsonc", b"{}", "empty", "")],
            &["custom/a.jsonc"],
        );
        invalid_utf8[12] = 0xff;
        assert!(decode(&invalid_utf8).unwrap_err().contains("UTF-8"));
        let mut trailing = empty.clone();
        trailing.push(0);
        assert!(decode(&trailing).unwrap_err().contains("trailing"));
        assert!(decode(&empty[..7]).is_err());
        let many: Vec<_> = (0..65)
            .map(|i| (format!("custom/{i:03}.jsonc"), b"{}".as_slice()))
            .collect();
        let records: Vec<_> = many
            .iter()
            .map(|(p, b)| (p.as_str(), *b, "empty", ""))
            .collect();
        let paths: Vec<_> = many.iter().map(|(p, _)| p.as_str()).collect();
        assert!(decode(&packet(&records, &paths))
            .unwrap_err()
            .contains("count"));
        let large = vec![b'x'; 256 * 1024];
        let many: Vec<_> = (0..17)
            .map(|i| (format!("custom/{i:03}.jsonc"), large.as_slice()))
            .collect();
        let records: Vec<_> = many
            .iter()
            .map(|(p, b)| (p.as_str(), *b, "empty", ""))
            .collect();
        let paths: Vec<_> = many.iter().map(|(p, _)| p.as_str()).collect();
        assert!(decode(&packet(&records, &paths))
            .unwrap_err()
            .contains("aggregate"));
    }
}
