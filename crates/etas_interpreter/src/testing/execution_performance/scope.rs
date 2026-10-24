use super::*;
use std::fmt::Write;

#[test]
fn checked_loop_scope_work_does_not_multiply_frame_width_by_iterations() {
    let mut baseline = None;
    for width in [1000, 2000, 4000] {
        let mut source = String::from("module app.main; flow main(input: Array<i32>");
        let mut args = vec![InterpValue::Array(vec![InterpValue::i32(7); 1000].into())];
        for index in 0..width {
            write!(source, ", unused{index}: i32").unwrap();
            args.push(InterpValue::i32(index));
        }
        source.push_str(") -> i32 { var count = 0; for x in input limit Iterations(1000) { count = count + 1; } return count; }");
        let program = PreparedExecution::new(&source);
        let (value, cost, elapsed) = program.run(&args);
        assert_eq!(value, InterpValue::i32(1000));
        eprintln!("checked scope width={width}, iterations=1000: {cost:?}, {elapsed:?}");
        if let Some((previous_width, previous_bytes)) = baseline {
            assert!(
                cost.bytes <= previous_bytes + (width - previous_width) as usize * 512,
                "each iteration rebuilt the frame's scope membership: {cost:?}"
            );
        }
        baseline = Some((width, cost.bytes));
    }
}
