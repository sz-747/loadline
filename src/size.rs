//! Human memory sizes. Binary units (1G = 1024^3 bytes), labelled "GB" like Task Manager.

const KB: u64 = 1024;
const MB: u64 = KB * 1024;
const GB: u64 = MB * 1024;

/// Parse sizes like `3G`, `3GB`, `1.5g`, `512M`, `512MB`. A bare number is bytes.
pub fn parse_size(input: &str) -> Result<u64, String> {
    let s = input.trim();
    let split = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let (number, unit) = s.split_at(split);
    let value: f64 = number
        .parse()
        .map_err(|_| format!("invalid size '{input}': expected e.g. 3G or 512M"))?;
    let multiplier = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "k" | "kb" | "kib" => KB,
        "m" | "mb" | "mib" => MB,
        "g" | "gb" | "gib" => GB,
        "t" | "tb" | "tib" => GB * 1024,
        other => return Err(format!("invalid size unit '{other}' in '{input}'")),
    };
    if !value.is_finite() || value <= 0.0 {
        return Err(format!("size must be positive: '{input}'"));
    }
    Ok((value * multiplier as f64).round() as u64)
}

pub fn format_size(bytes: u64) -> String {
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{} MB", bytes / MB)
    } else {
        format!("{} KB", bytes / KB)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_forms() {
        assert_eq!(parse_size("3G").unwrap(), 3 * GB);
        assert_eq!(parse_size("3gb").unwrap(), 3 * GB);
        assert_eq!(parse_size("1.5G").unwrap(), 3 * GB / 2);
        assert_eq!(parse_size("512M").unwrap(), 512 * MB);
        assert_eq!(parse_size(" 512 MB ").unwrap(), 512 * MB);
        assert_eq!(parse_size("4096").unwrap(), 4096);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse_size("").is_err());
        assert!(parse_size("G").is_err());
        assert!(parse_size("3X").is_err());
        assert!(parse_size("0G").is_err());
    }

    #[test]
    fn formats() {
        assert_eq!(format_size(3 * GB), "3.0 GB");
        assert_eq!(format_size(300 * MB), "300 MB");
        assert_eq!(format_size(2 * KB), "2 KB");
    }
}
