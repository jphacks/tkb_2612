//! CSVパーサが受け付けてしまう構文の不正の検出。
//!
//! `csv` クレート（Pythonの `csv` も同じ）は、引用符で始めたフィールドが閉じないまま
//! ファイルが終わってもエラーにせず、残りの全行を1つのフィールドとして返す。
//! これでは行が黙って失われるので、前処理と祝日表の読込ではCSV構文の不正として失敗させる
//! （behavior.md 2章「CSV構文…は前処理全体の失敗」）。

/// 引用符で始めたフィールドの途中でテキストが終わっているか。
///
/// フィールドの先頭の `"` で引用を始め、引用中の `""` は1文字の `"`、引用を閉じた後の
/// 文字は同じフィールドの続きとして扱う（`csv` クレートと同じ解釈）。
pub fn ends_inside_quoted_field(text: &str) -> bool {
    #[derive(Clone, Copy, PartialEq)]
    enum State {
        FieldStart,
        Unquoted,
        Quoted,
        QuoteInQuoted,
    }
    let mut state = State::FieldStart;
    for b in text.bytes() {
        state = match (state, b) {
            (State::Quoted, b'"') => State::QuoteInQuoted,
            (State::Quoted, _) => State::Quoted,
            (State::QuoteInQuoted, b'"') => State::Quoted,
            (State::FieldStart, b'"') => State::Quoted,
            (_, b',' | b'\n' | b'\r') => State::FieldStart,
            _ => State::Unquoted,
        };
    }
    state == State::Quoted
}
