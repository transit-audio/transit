use serde_json::json;
use transit_engine::{Engine, TransportRequest};
use transit_model::TransportControl;

#[test]
fn explicit_transport_from_is_one_exact_start_request() {
    let mut engine = Engine::default();
    engine.execute("transport.play", &json!({"from": 98765})).unwrap();
    assert_eq!(engine.transport_requests, [TransportRequest::PlayExact(98765)]);
}

#[test]
fn transport_learn_and_map_refuse_session_before_mutation_and_survive_session_change() {
    let mut e = Engine::default();
    for t in TransportControl::ALL {
        let target = t.target().to_string();
        for command in ["midi.learn", "midi.map"] {
            let err = e.execute(command, &json!({"target": target, "scope": "session"})).unwrap_err();
            assert!(err.to_string().contains(transit_model::midi_transport::MACHINE_ONLY));
            assert!(e.session().midi_mappings.is_empty());
            assert!(e.learn().is_none());
        }
        e.execute("midi.learn", &json!({"target": target, "scope":"profile"})).unwrap();
        e.execute("midi.map", &json!({"target": target, "scope":"profile", "source":{"port":"Controller", "kind":"cc", "channel":1,"number":30}}))
            .unwrap();
        assert_eq!(e.midi_transport()["Controller"].mappings[0].target, t.target());
    }
    let before = e.midi_transport().clone();
    e.execute("session.new", &json!({})).unwrap();
    assert_eq!(e.midi_transport(), &before);
}
