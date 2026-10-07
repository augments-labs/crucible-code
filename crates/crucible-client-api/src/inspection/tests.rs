//! What an inspection document says, and what it refuses to say.

use serde_json::{Value, json};

use super::*;
use crate::bounds::TEXT_BYTES;

fn name(word: &str) -> Name {
    Name::new(word).unwrap()
}

fn plan(enabled: bool) -> Plan {
    Plan {
        enabled,
        policy: Digest([0x11; 32]),
        cwd: Digest([0x22; 32]),
        roots: vec![Root {
            access: name("read_write"),
            provenance: name("workspace_root"),
            identity: Digest([0x33; 32]),
        }],
        omitted: 0,
        hidden: 4,
        network: Network::Closed,
        ceilings: vec![
            Ceiling {
                feature: name("command_time_limit"),
                amount: 90,
                nanos: 500,
                unit: Unit::Seconds,
                claim: Claim::Enforced,
            },
            Ceiling {
                feature: name("output_limit"),
                amount: 1 << 20,
                nanos: 0,
                unit: Unit::Bytes,
                claim: Claim::Observed,
            },
            Ceiling {
                feature: name("process_limit"),
                amount: 64,
                nanos: 0,
                unit: Unit::Count,
                claim: Claim::Unsupported,
            },
            Ceiling {
                feature: name("cost_limit"),
                amount: 7,
                nanos: 0,
                unit: Unit::Micros,
                claim: Claim::Unsupported,
            },
        ],
        commands: Digest([0x44; 32]),
        staged: 0,
        persistent: false,
        snapshots: false,
    }
}

fn backend(provenance: &str, version: BackendVersion) -> Backend {
    Backend {
        name: name("a-backend"),
        version,
        provenance: name(provenance),
        build: (provenance != COMPATIBILITY).then_some(Digest([0x5a; 32])),
        capabilities: vec![
            Capability {
                feature: name("filesystem"),
                claim: Claim::Enforced,
            },
            Capability {
                feature: name("usage"),
                claim: Claim::Observed,
            },
            Capability {
                feature: name("memory_limit"),
                claim: Claim::Unsupported,
            },
        ],
    }
}

/// One document for every status, and between them every field.
pub(crate) fn inspections(marked: &Text) -> Vec<Inspection> {
    let ready = Inspected {
        enabled: true,
        mode: Requirement::Optional,
        requested: plan(true),
        effective: plan(true),
        backend: Some(backend(
            "system",
            BackendVersion::Unverified(marked.clone()),
        )),
        unchecked: Some(marked.clone()),
        refusal: None,
        confined: true,
    };
    let disabled = Inspected {
        enabled: false,
        mode: Requirement::Optional,
        requested: plan(false),
        effective: plan(false),
        backend: Some(backend(COMPATIBILITY, BackendVersion::Stated(name("1")))),
        unchecked: None,
        refusal: None,
        confined: false,
    };
    let refused = Inspected {
        effective: Plan {
            network: Network::Domains {
                allowed: 2,
                denied: 1,
                local_binding: true,
                unix_sockets: 0,
            },
            ..plan(true)
        },
        refusal: Some(marked.clone()),
        confined: false,
        ..ready.clone()
    };
    let unavailable = Inspected {
        mode: Requirement::Required,
        backend: None,
        unchecked: None,
        refusal: Some(marked.clone()),
        confined: false,
        ..ready.clone()
    };
    vec![
        Inspection::Inspected(Box::new(ready)),
        Inspection::Inspected(Box::new(disabled)),
        Inspection::Inspected(Box::new(refused)),
        Inspection::Inspected(Box::new(unavailable)),
        Inspection::Failed(marked.clone()),
    ]
}

fn plain() -> Text {
    Text::cut("words")
}

/// The specimen at `at`, as a document to edit.
fn document(at: usize) -> Value {
    value(inspections(&plain()).get(at).unwrap())
}

/// The report at `at`, as a value to edit.
fn report(at: usize) -> Inspected {
    let Inspection::Inspected(inspected) = inspections(&plain()).swap_remove(at) else {
        panic!("specimen {at} is a report");
    };
    *inspected
}

/// The document `inspection` is.
fn value(inspection: &Inspection) -> Value {
    serde_json::from_slice(&inspection.encode().unwrap()).unwrap()
}

/// The field at `pointer`.
fn at<'a>(document: &'a Value, pointer: &str) -> &'a Value {
    document.pointer(pointer).unwrap()
}

/// `document` with the field at `pointer` set to `to`, adding it where absent.
fn with(mut document: Value, pointer: &str, to: Value) -> Value {
    let (parent, key) = pointer.rsplit_once('/').unwrap();
    document
        .pointer_mut(parent)
        .and_then(Value::as_object_mut)
        .unwrap()
        .insert(key.to_owned(), to);
    document
}

/// What decoding `document` is refused with.
fn refused(document: &Value) -> ErrorCode {
    Inspection::decode(&serde_json::to_vec(document).unwrap())
        .unwrap_err()
        .code()
}

#[test]
fn every_document_is_one_line_naming_its_format_kind_and_status() {
    let statuses: Vec<&str> = inspections(&plain())
        .iter()
        .map(Inspection::status)
        .collect();
    assert_eq!(
        statuses,
        ["ready", "ready", "refused", "unavailable", "failed"]
    );

    for inspection in inspections(&plain()) {
        let written = inspection.encode().unwrap();
        let line = written.strip_suffix(b"\n").unwrap();
        assert!(!line.contains(&b'\n'));
        let document = value(&inspection);
        assert_eq!(at(&document, "/format_version"), &json!(1));
        assert_eq!(at(&document, "/kind"), &json!("sandbox-inspection"));
        assert_eq!(at(&document, "/status"), &json!(inspection.status()));
        assert_eq!(at(&document, "/truncated"), &json!(false));
        assert_eq!(Inspection::decode(&written).unwrap(), inspection);
    }
}

#[test]
fn another_format_version_is_refused_by_name() {
    for version in [0, 2, u64::MAX] {
        let other = with(document(0), "/format_version", json!(version));
        assert_eq!(refused(&other), ErrorCode::UnsupportedVersion);
    }
}

#[test]
fn an_ordinary_subprocess_is_never_called_confined() {
    let claimed = Inspection::Inspected(Box::new(Inspected {
        enabled: true,
        effective: plan(true),
        confined: true,
        ..report(1)
    }));
    assert_eq!(claimed.encode().unwrap_err().code(), ErrorCode::Malformed);

    let claimed = with(document(1), "/confined", json!(true));
    let claimed = with(claimed, "/effective/enabled", json!(true));
    assert_eq!(refused(&claimed), ErrorCode::Malformed);
}

#[test]
fn confined_is_said_only_of_a_backend_found_that_takes_an_enabled_policy() {
    for specimen in [2, 3] {
        let claimed = with(document(specimen), "/confined", json!(true));
        assert_eq!(refused(&claimed), ErrorCode::Malformed, "{specimen}");
    }

    let unconfined = with(document(0), "/effective/enabled", json!(false));
    assert_eq!(refused(&unconfined), ErrorCode::Malformed);

    // A missing backend has to say why it is missing.
    let mut silent = document(3);
    silent.as_object_mut().unwrap().remove("refusal");
    assert_eq!(refused(&silent), ErrorCode::Malformed);
}

#[test]
fn a_status_that_disagrees_with_the_report_is_refused() {
    for (specimen, status) in [(0, "refused"), (2, "ready"), (3, "ready"), (4, "ready")] {
        let other = with(document(specimen), "/status", json!(status));
        assert_eq!(
            refused(&other),
            ErrorCode::Malformed,
            "{specimen} as {status}"
        );
    }
}

#[test]
fn what_was_cut_or_left_out_is_said_at_the_top() {
    let long = Text::cut(&"é".repeat(TEXT_BYTES));
    assert!(long.truncated());
    // The disabled report carries no words, so nothing in it can be cut.
    for (specimen, inspection) in inspections(&long).into_iter().enumerate() {
        let document = value(&inspection);
        if specimen == 1 {
            assert_eq!(at(&document, "/truncated"), &json!(false));
            continue;
        }
        assert_eq!(
            at(&document, "/truncated"),
            &json!(true),
            "{}",
            inspection.status()
        );
        assert_eq!(
            Inspection::decode(&inspection.encode().unwrap()).unwrap(),
            inspection
        );

        let hidden = with(document, "/truncated", json!(false));
        assert_eq!(refused(&hidden), ErrorCode::Malformed);
    }

    let mut many = report(1);
    many.effective.omitted = 3;
    let document = value(&Inspection::Inspected(Box::new(many)));
    assert_eq!(at(&document, "/truncated"), &json!(true));
}

#[test]
fn a_list_over_its_ceiling_is_refused_rather_than_cut() {
    let mut many = report(0);
    let root = many.requested.roots.first().unwrap().clone();
    many.requested.roots = vec![root; ITEMS + 1];
    let over = Inspection::Inspected(Box::new(many));
    assert_eq!(over.encode().unwrap_err().code(), ErrorCode::TooLarge);

    let good = document(0);
    let root = at(&good, "/requested/roots/0").clone();
    let over = with(
        good,
        "/requested/roots",
        Value::Array(vec![root; ITEMS + 1]),
    );
    assert_eq!(refused(&over), ErrorCode::TooLarge);
}

#[test]
fn a_digest_is_sixty_four_lowercase_digits_and_nothing_else() {
    let good = document(0);
    assert_eq!(at(&good, "/effective/cwd"), &json!("22".repeat(32)));
    for digits in [
        "2".repeat(63),
        "2".repeat(65),
        "AB".repeat(32),
        format!("+f{}", "22".repeat(31)),
        format!("g0{}", "22".repeat(31)),
    ] {
        let other = with(good.clone(), "/effective/cwd", json!(digits));
        assert_eq!(refused(&other), ErrorCode::Malformed, "{digits}");
    }
}

#[test]
fn nanoseconds_cross_only_beside_seconds() {
    let mut odd = report(0);
    odd.effective.ceilings.get_mut(1).unwrap().nanos = 5;
    assert_eq!(
        Inspection::Inspected(Box::new(odd))
            .encode()
            .unwrap_err()
            .code(),
        ErrorCode::Malformed
    );

    for nanos in [json!(0), json!(1_000_000_000), json!(u64::MAX)] {
        let other = with(document(0), "/effective/ceilings/0/nanos", nanos.clone());
        assert_eq!(refused(&other), ErrorCode::Malformed, "{nanos}");
    }
}

#[test]
fn a_field_nobody_asked_for_refuses_the_document() {
    for specimen in 0..5 {
        let other = with(document(specimen), "/extra", json!(1));
        assert_eq!(refused(&other), ErrorCode::Malformed, "{specimen}");
    }
    let failed = with(document(4), "/enabled", json!(true));
    assert_eq!(refused(&failed), ErrorCode::Malformed);
}
