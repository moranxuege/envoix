use super::*;

#[test]
fn render_output_contains_only_block_chars_and_newlines() {
    let qr = render_terminal_qr("envoix://invite/v2/opaque").unwrap();
    assert!(
        qr.chars()
            .all(|ch| matches!(ch, '█' | '▀' | '▄' | ' ' | '\n'))
    );
}

#[test]
fn render_all_lines_have_equal_width() {
    let qr = render_terminal_qr("envoix://invite/v2/opaque").unwrap();
    let lines = qr.trim_end_matches('\n').split('\n').collect::<Vec<_>>();
    let widths = lines
        .iter()
        .map(|line| line.chars().count())
        .collect::<Vec<_>>();
    assert!(widths.windows(2).all(|pair| pair[0] == pair[1]));
}
