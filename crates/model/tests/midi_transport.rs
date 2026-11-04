use transit_model::{ControlTarget, Mapping, MappingSet, MidiSource, Session, SourceKind, TransportControl, TransportPrefs, TransportProfiles};

fn mapping(target: TransportControl) -> Mapping {
    Mapping::new(MidiSource { port: "Controller".into(), channel: Some(0), kind: SourceKind::Cc, number: 20 }, target.target())
}

#[test]
fn all_transport_targets_round_trip_only_in_machine_store() {
    for target in TransportControl::ALL {
        let text = format!("transport:{}", target.key());
        assert_eq!(text.parse::<ControlTarget>().unwrap(), target.target());
        let mut profiles = TransportProfiles::new();
        profiles.entry("Controller".into()).or_default().set(mapping(target)).unwrap();
        let encoded = serde_json::to_string(&profiles).unwrap();
        assert_eq!(serde_json::from_str::<TransportProfiles>(&encoded).unwrap(), profiles);
        let mut session = MappingSet::default();
        assert_eq!(session.set(mapping(target)).unwrap_err(), transit_model::midi_transport::MACHINE_ONLY);
        assert!(session.is_empty());
    }
    let prefs: TransportPrefs = serde_json::from_str("{}").unwrap();
    assert!(prefs.transport_buttons);
    assert_eq!(prefs.mmc_device_id, 0);
    assert!(prefs.mappings.is_empty());
    assert!(!serde_json::from_str::<TransportPrefs>(r#"{"transport_buttons":false}"#).unwrap().transport_buttons);
    assert!(serde_json::from_str::<TransportPrefs>(r#"{"mmc_device_id":127}"#).is_err());
}

#[test]
fn imported_transport_mapping_warns_and_never_enters_session_or_saved_file() {
    let mut value = serde_json::to_value(Session::default()).unwrap();
    value["midi_mappings"] = serde_json::json!([mapping(TransportControl::Record)]);
    let loaded = Session::from_json(&value.to_string()).unwrap();
    assert!(loaded.midi_mappings.is_empty());
    assert_eq!(loaded.load_notes, [transit_model::midi_transport::MACHINE_ONLY]);
    assert!(!loaded.to_json().unwrap().contains("transport:record"));
    let set: MappingSet = serde_json::from_value(value["midi_mappings"].clone()).unwrap();
    assert!(set.is_empty(), "direct serde cannot bypass the store invariant");
}
