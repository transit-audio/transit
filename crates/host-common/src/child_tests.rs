use super::*;

fn reply_line(json: &str) -> Vec<u8> {
    format!("{REPLY}{json}\n").into_bytes()
}

#[test]
fn the_first_complete_answer_line_is_kept_and_other_lines_are_dropped() {
    let mut c = Collector::new(64, 1024);
    c.push(b"plug-in chatter\n").unwrap();
    assert!(c.line.is_empty(), "a finished line that is not an answer is not kept");
    c.push(&reply_line("{\"a\":1}")[..10]).unwrap();
    assert_eq!(c.reply(), None, "half a line is no answer");
    c.push(&reply_line("{\"a\":1}")[10..]).unwrap();
    assert_eq!(c.reply(), Some("{\"a\":1}"));
    c.push(&reply_line("{\"b\":2}")).unwrap();
    assert_eq!(c.reply(), Some("{\"a\":1}"), "the first answer stands");
    let mut d = Collector::new(64, 1024);
    d.push(format!("{REPLY}{{}}\r\n").as_bytes()).unwrap();
    assert_eq!(d.reply(), Some("{}"), "a carriage return is not part of the answer");
    let mut e = Collector::new(64, 1024);
    e.push(format!("{REPLY}{{\"last\":true}}").as_bytes()).unwrap();
    assert_eq!(e.reply(), None);
    e.finish();
    assert_eq!(e.reply(), Some("{\"last\":true}"), "an unterminated last line counts at the end of the output");
}

#[test]
fn a_line_past_its_bound_is_an_error_not_more_memory() {
    let mut c = Collector::new(16, 1024);
    c.push(&[b'x'; 10]).unwrap();
    let e = c.push(&[b'x'; 10]).unwrap_err();
    assert!(e.contains("longer than 16 bytes"), "{e}");
    assert!(c.line.len() <= 16, "{}", c.line.len());
    // Lines that end within the bound are fine, however many.
    let mut d = Collector::new(16, 1 << 20);
    for _ in 0..1000 {
        d.push(b"0123456789\n").unwrap();
    }
    assert!(d.line.is_empty());
}

#[test]
fn output_past_its_bound_is_an_error() {
    let mut c = Collector::new(1024, 100);
    for _ in 0..9 {
        c.push(b"012345678\n").unwrap();
    }
    let e = c.push(b"0123456789\n").unwrap_err();
    assert!(e.contains("passed 100 bytes"), "{e}");
}
