use super::*;

fn frame_with_four_tablets() -> DataFrame {
    DataFrame {
        hwnd: HWND(0),
        himetric_to_logical_pixel: 1.0,
        stylus_states: std::collections::BTreeMap::new(),
        tools: Vec::new(),
        raw_tablet_deletions: Vec::new(),
        raw_tablets: [10, 20, 30, 40]
            .into_iter()
            .map(|tcid| RawTabletSlot::Dummy { tcid })
            .collect(),
        tablets: Vec::new(),
        events: Vec::new(),
    }
}

#[test]
fn removing_two_tablets_between_pumps_preserves_the_other_slots() {
    let mut frame = frame_with_four_tablets();
    frame.delete_tcid_by_idx(1).unwrap();
    frame.delete_tcid_by_idx(1).unwrap();
    frame.frame_end_cleanup();
    assert_eq!(
        frame
            .raw_tablets
            .iter()
            .map(RawTabletSlot::tcid)
            .collect::<Vec<_>>(),
        [10, 40]
    );
}

#[test]
fn removing_the_last_two_tablets_between_pumps_does_not_panic() {
    let mut frame = frame_with_four_tablets();
    frame.delete_tcid_by_idx(2).unwrap();
    frame.delete_tcid_by_idx(2).unwrap();
    frame.frame_end_cleanup();
    assert_eq!(
        frame
            .raw_tablets
            .iter()
            .map(RawTabletSlot::tcid)
            .collect::<Vec<_>>(),
        [10, 20]
    );
}
