use super::*;

#[test]
fn a_run_event_is_written_with_its_name_and_reads_back() {
    let event = RunEvent::Finished {
        run_id: 7,
        id: "t3".into(),
        status: Status::Failed,
        duration_ms: 12,
        message: "boom".into(),
    };

    let text = to_payload(&event);

    assert_eq!(
        text,
        r#"{"event":"finished","run_id":7,"id":"t3","status":"failed","duration_ms":12,"message":"boom"}"#
    );
    assert_eq!(from_payload::<RunEvent>(&text), Some(event));
}

#[test]
fn an_event_or_status_a_later_adapter_has_reads_as_unknown_not_as_an_error() {
    assert_eq!(
        from_payload::<RunEvent>(r#"{"event":"retrying","run_id":1}"#),
        Some(RunEvent::Unknown)
    );
    assert_eq!(
        from_payload::<RunEvent>(r#"{"event":"finished","run_id":1,"id":"a","status":"flaky"}"#),
        Some(RunEvent::Finished {
            run_id: 1,
            id: "a".into(),
            status: Status::Unknown,
            duration_ms: 0,
            message: String::new(),
        })
    );
    assert_eq!(RunEvent::Unknown.run_id(), None);
}

#[test]
fn a_discover_request_without_a_size_asks_for_a_whole_page() {
    let args: DiscoverArgs = from_payload(r#"{"offset":500}"#).unwrap();

    assert_eq!(args, DiscoverArgs::page_at(500));
}

#[test]
fn a_run_without_ids_runs_everything_and_nothing_else_is_assumed() {
    let args: RunArgs = from_payload(r#"{"run_id":2}"#).unwrap();

    assert_eq!(args.ids, None);
    assert!(
        from_payload::<RunArgs>(r#"{"ids":["a"]}"#).is_none(),
        "no run id"
    );
}

#[test]
fn a_scenario_says_only_what_differs_from_the_plain_adapter() {
    let scenario: Scenario = from_payload(r#"{"tests":20000,"same_names":true}"#).unwrap();

    assert_eq!(scenario.tests, 20000);
    assert!(scenario.same_names);
    assert!(!scenario.traps_on_advance && !scenario.talks_after_cancel);
    assert_eq!(from_payload::<Scenario>("{}"), Some(Scenario::default()));
}

#[test]
fn what_is_not_the_payload_asked_for_is_none() {
    assert_eq!(from_payload::<Page>("nonsense"), None);
}

#[test]
fn a_page_that_failed_says_why_and_a_good_page_does_not_mention_an_error() {
    let failed = Page::failed("it does not build");
    let text = to_payload(&failed);

    assert_eq!(
        text,
        r#"{"offset":0,"total":0,"tests":[],"error":"it does not build"}"#
    );
    assert_eq!(from_payload::<Page>(&text), Some(failed));
    let good = Page {
        offset: 0,
        total: 0,
        tests: Vec::new(),
        error: None,
    };
    assert_eq!(to_payload(&good), r#"{"offset":0,"total":0,"tests":[]}"#);
    assert_eq!(
        from_payload::<Page>(r#"{"offset":0,"total":0,"tests":[]}"#),
        Some(good)
    );
}

#[test]
fn an_error_event_belongs_to_its_run_like_the_others() {
    let event = RunEvent::Error {
        run_id: 4,
        message: "no build".into(),
    };

    assert_eq!(event.run_id(), Some(4));
    assert_eq!(
        to_payload(&event),
        r#"{"event":"error","run_id":4,"message":"no build"}"#
    );
}
