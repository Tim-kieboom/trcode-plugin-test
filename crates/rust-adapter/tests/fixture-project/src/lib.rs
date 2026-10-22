pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_empty() {
        assert_eq!(super::add(1, 1), 2);
    }

    #[test]
    fn fails_here() {
        assert_eq!(super::add(1, 1), 3);
    }

    #[test]
    #[should_panic]
    fn panics_ok() {
        panic!("expected");
    }

    #[test]
    #[ignore]
    fn ignored_one() {}
}
