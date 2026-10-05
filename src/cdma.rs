pub fn encode_text(text: &str, code: &[i8]) -> Vec<i32> {
    let mut signal = Vec::with_capacity(text.len() * 8 * code.len());
    for byte in text.bytes() {
        for bit in (0..8).rev() {
            let data = if byte & (1 << bit) == 0 { -1 } else { 1 };
            signal.extend(code.iter().map(|chip| data * *chip as i32));
        }
    }
    signal
}

pub fn decode_text(signal: &[i32], code: &[i8]) -> Result<String, String> {
    if code.is_empty() || signal.len() % (code.len() * 8) != 0 {
        return Err("invalid CDMA signal".to_string());
    }
    let mut bytes = Vec::with_capacity(signal.len() / (code.len() * 8));
    for byte_signal in signal.chunks(code.len() * 8) {
        let mut byte = 0u8;
        for bit_signal in byte_signal.chunks(code.len()) {
            let value: i32 = bit_signal
                .iter()
                .zip(code)
                .map(|(sample, chip)| sample * *chip as i32)
                .sum();
            if value == 0 {
                return Err("signal could not be decoded".to_string());
            }
            byte = (byte << 1) | u8::from(value > 0);
        }
        bytes.push(byte);
    }
    String::from_utf8(bytes).map_err(|_| "decoded message is not valid text".to_string())
}

pub fn combine(signals: &[Vec<i32>]) -> Vec<i32> {
    let length = signals.iter().map(Vec::len).max().unwrap_or(0);
    let mut combined = vec![0; length];
    for signal in signals {
        for (position, sample) in signal.iter().enumerate() {
            combined[position] += sample;
        }
    }
    combined
}

pub fn format_signal(signal: &[i32]) -> String {
    signal
        .iter()
        .map(i32::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

pub fn parse_signal(value: &str) -> Option<Vec<i32>> {
    if value.is_empty() {
        return Some(Vec::new());
    }
    value
        .split(',')
        .map(|sample| sample.parse::<i32>().ok())
        .collect()
}
