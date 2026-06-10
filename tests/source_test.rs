use exx::source::SourceHub;

#[test]
fn line_starts() {
    let mut shub = SourceHub::new();

    let src = {
        let text = "
blabla


bla
bla

    abc
        ";
        shub.add_source("".into(), text.to_owned())
    };
    assert_eq!(*src.line_starts(), vec![0, 1, 8, 9, 10, 14, 18, 19, 27]);

    // avec \r et \r\n
    let src = {
        let text = "blabla\rsalut\r\nbonjour\r\n\r\nlol";
        shub.add_source("".into(), text.to_owned())
    };
    assert_eq!(*src.line_starts(), vec![0, 7, 14, 23, 25]);
}
