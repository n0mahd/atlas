/// Plate numbers in Roman numerals, as in a printed atlas.
#[must_use]
pub fn roman(mut n: usize) -> String {
    const TABLE: [(usize, &str); 13] = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut out = String::new();
    for (value, letters) in TABLE {
        while n >= value {
            out.push_str(letters);
            n -= value;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn numerals() {
        for (n, r) in [
            (1, "I"),
            (4, "IV"),
            (9, "IX"),
            (14, "XIV"),
            (88, "LXXXVIII"),
            (2026, "MMXXVI"),
        ] {
            assert_eq!(super::roman(n), r);
        }
    }
}
