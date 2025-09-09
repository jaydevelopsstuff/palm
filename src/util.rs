pub fn hex_encode_formatted<T: AsRef<[u8]>>(data: T) -> String {
    hex::encode_upper(data)
        .chars()
        .enumerate()
        .flat_map(|(i, c)| {
            if i != 0 && i % 2 == 0 {
                Some(' ')
            } else {
                None
            }
            .into_iter()
            .chain(std::iter::once(c))
        })
        .collect::<String>()
}
