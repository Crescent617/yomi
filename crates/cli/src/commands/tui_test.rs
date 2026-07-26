use super::TuiArgs;
use clap::Parser;

#[test]
fn daemon_flag_defaults_off() {
    let args = TuiArgs::try_parse_from(["yomi"]).unwrap();
    assert!(!args.daemon);
}

#[test]
fn bg_alias_enables_daemon() {
    let args = TuiArgs::try_parse_from(["yomi", "--bg"]).unwrap();
    assert!(args.daemon);
}

#[test]
fn fg_flag_is_rejected() {
    assert!(TuiArgs::try_parse_from(["yomi", "--fg"]).is_err());
}
