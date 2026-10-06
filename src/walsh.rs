pub fn codes(users: usize) -> Vec<Vec<i8>> {
    if users == 0 {
        return Vec::new();
    }
    let order = users.next_power_of_two();
    let mut matrix = vec![vec![1i8]];
    while matrix.len() < order {
        let size = matrix.len();
        let mut next = vec![vec![0i8; size * 2]; size * 2];
        for row in 0..size {
            for column in 0..size {
                let value = matrix[row][column];
                next[row][column] = value;
                next[row][column + size] = value;
                next[row + size][column] = value;
                next[row + size][column + size] = -value;
            }
        }
        matrix = next;
    }
    matrix.into_iter().take(users).collect()
}

pub fn format(code: &[i8]) -> String {
    code.iter()
        .map(|chip| if *chip == 1 { "+1" } else { "-1" })
        .collect::<Vec<_>>()
        .join(",")
}

pub fn parse(value: &str) -> Option<Vec<i8>> {
    let code: Option<Vec<i8>> = value
        .split(',')
        .map(|chip| match chip.trim() {
            "+1" | "1" => Some(1),
            "-1" => Some(-1),
            _ => None,
        })
        .collect();
    code.filter(|code| !code.is_empty())
}

pub fn display(code: &[i8]) -> String {
    format!("[{}]", format(code).replace(',', " "))
}

pub fn resize(code: &[i8], length: usize) -> Vec<i8> {
    code.iter().cycle().take(length).copied().collect()
}
