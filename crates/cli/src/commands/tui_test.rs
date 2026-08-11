use super::TuiArgs;
use clap::Parser;

#[test]
fn kernel_flags_default_off() {
    let args = TuiArgs::try_parse_from(["yomi"]).unwrap();
    assert!(!args.global.bg);
    assert!(!args.global.fg);
}

#[test]
fn bg_flag_is_accepted() {
    let args = TuiArgs::try_parse_from(["yomi", "--bg"]).unwrap();
    assert!(args.global.bg);
}

#[test]
fn fg_flag_is_accepted() {
    let args = TuiArgs::try_parse_from(["yomi", "--fg"]).unwrap();
    assert!(args.global.fg);
}

#[test]
fn bg_and_fg_conflict() {
    assert!(TuiArgs::try_parse_from(["yomi", "--bg", "--fg"]).is_err());
}
