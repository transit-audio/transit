//! `audio.fake_devices`: the stand-in devices of screenshot runs and tests, read from JSON (moved
//! out of `audio_devices.rs` unchanged).

use super::FakeDevices;
use transit_playback::AudioConfig;
use transit_playback::devices::{AudioSystem, DeviceInfo};

/// `audio.fake_devices {systems: [{name, is_default?, outputs: [...], inputs: [...], diagnostics?: [...]}]}`:
/// stand-in devices for screenshot runs and tests (each device `{name, is_default?, max_channels?,
/// sample_rates?, buffer_min?, buffer_max?, channel_names?}`, each diagnostic a "<driver>: <problem>" text; a JACK system's
/// `virtual_inputs` are devices too); `{systems: null}` removes them.
pub fn fake_from_json(p: &serde_json::Value) -> Result<Option<FakeDevices>, String> {
    use serde_json::Value;
    let Some(list) = p.get("systems") else { return Err("audio.fake_devices needs systems".into()) };
    if list.is_null() {
        return Ok(None);
    }
    let Some(list) = list.as_array() else { return Err("systems must be a list".into()) };
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let flag = |v: &Value, k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
    let devs = |v: &Value, k: &str| -> Result<Vec<DeviceInfo>, String> {
        v.get(k)
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|d| {
                Ok(DeviceInfo {
                    name: text(d, "name").ok_or("every device needs a name")?,
                    is_default: flag(d, "is_default"),
                    max_channels: d.get("max_channels").and_then(Value::as_u64).and_then(|n| u16::try_from(n).ok()).unwrap_or(2),
                    sample_rates: d
                        .get("sample_rates")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().filter_map(|r| r.as_u64().and_then(|r| u32::try_from(r).ok())).collect())
                        .unwrap_or_else(|| vec![44_100, 48_000]),
                    buffer_min: Some(d.get("buffer_min").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()).unwrap_or(64)),
                    buffer_max: Some(d.get("buffer_max").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()).unwrap_or(2048)),
                    channel_names: d
                        .get("channel_names")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().filter_map(|n| n.as_str().map(str::to_string)).collect())
                        .unwrap_or_default(),
                })
            })
            .collect()
    };
    let systems = list
        .iter()
        .map(|s| {
            Ok(AudioSystem {
                name: text(s, "name").ok_or("every system needs a name")?,
                is_default: flag(s, "is_default"),
                outputs: devs(s, "outputs")?,
                inputs: devs(s, "inputs")?,
                diagnostics: s
                    .get("diagnostics")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(|d| d.as_str().map(str::to_string)).collect())
                    .unwrap_or_default(),
                virtual_inputs: devs(s, "virtual_inputs")?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut fake = FakeDevices { systems, ..Default::default() };
    fake.connection = Some(crate::audio_connection::FakeConnection::new(AudioConfig::default(), std::rc::Rc::clone(&fake.trace)));
    Ok(Some(fake))
}
