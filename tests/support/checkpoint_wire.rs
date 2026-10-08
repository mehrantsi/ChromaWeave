#[cfg(test)]
mod release_checkpoint_wire {
    use super::*;
    #[allow(dead_code)]
    mod cases {
        use crate as chromaweave;
        include!(env!("CW_RELEASE_CASES"));
    }
    #[test]
    fn checkpoint_private_codewords_match_distributable_wire() {
        for case in cases::optical_cases() {
            let encoded =
                encode_with_options(&case.payload, 4, case.modulation, case.options).unwrap();
            let payload = if encoded.layout.compression == "zstd" {
                crate::compression::compress(&case.payload).unwrap()
            } else {
                case.payload
            };
            let actual: Vec<u8> = codewords(&payload, &encoded.layout)
                .into_iter()
                .flatten()
                .collect();
            let expected = std::fs::read(
                std::path::Path::new(&std::env::var("CW_CORPUS_OUTPUT").unwrap())
                    .join(format!("{}.wire", case.name)),
            )
            .unwrap();
            assert_eq!(
                actual, expected,
                "{} private checkpoint codewords",
                case.name
            );
        }
    }
}
