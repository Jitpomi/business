use super::*;
#[test]
fn encrypted_recovery_requires_original_key_and_rejects_tampering_without_output() -> Result<()> {
    let d = tempfile::tempdir()?;
    let input = d.path().join("input");
    let archive = d.path().join("backup");
    let key = d.path().join("key");
    let output = d.path().join("output");
    std::fs::write(&input, b"credentials and business work")?;
    encrypt(&input, &archive, &key)?;
    decrypt(&archive, &output, &key)?;
    assert_eq!(std::fs::read(&output)?, std::fs::read(&input)?);
    assert!(encrypt(&input, &archive, &key).is_err());
    let wrong = d.path().join("wrong-key");
    use base64::Engine;
    std::fs::write(
        &wrong,
        format!(
            "JITPOMI-RECOVERY-KEY-V1\n{}\n",
            base64::engine::general_purpose::STANDARD.encode([1u8; 32])
        ),
    )?;
    let bad = d.path().join("bad-output");
    assert!(decrypt(&archive, &bad, &wrong).is_err());
    assert!(!bad.exists());
    let mut bytes = std::fs::read(&archive)?;
    *bytes.last_mut().unwrap() ^= 1;
    std::fs::write(&archive, bytes)?;
    assert!(decrypt(&archive, &bad, &key).is_err());
    assert!(!bad.exists());
    Ok(())
}
