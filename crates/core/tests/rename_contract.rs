//! リネーム後の名前・形式の契約を固定する（アプリ `Ligrol` / 形式 `nartik`）。
//!
//! AAD や keyring のスロット名は、間違えても「エラーにならず静かに読めなくなる」
//! 種類の値なので、文字列そのものをテストで固定する。

#[test]
fn pack_format_identity_is_nartik() {
    assert_eq!(&nartik::MAGIC, b"NRTK", "パックのマジック");
    assert_eq!(
        nartik::APP_SALT,
        b"nartik-v1-identity-salt-2026",
        "`sub` ラップの KEK に使う salt"
    );
}

#[test]
fn envelope_labels_are_ligrol() {
    let backup = nartik::BACKUP_LABEL;
    assert_eq!(backup.aad_prefix, "ligrol-backup", "バックアップの AAD");
    assert_eq!(backup.kdf_salt, b"ligrol-backup:v1", "バックアップの salt");
    assert_eq!(
        backup.cipher_info,
        b"ligrol-backup-key",
        "バックアップの暗号鍵 info"
    );
    assert_eq!(
        backup.hash_info,
        b"ligrol-backup-hash",
        "バックアップの HMAC 鍵 info"
    );

    let thumbs = nartik::THUMBS_LABEL;
    assert_eq!(thumbs.aad_prefix, "ligrol-thumbs", "表紙バンドルの AAD");
    assert_eq!(thumbs.kdf_salt, b"ligrol-thumbs:v1", "表紙バンドルの salt");
}

#[test]
fn local_identity_is_ligrol() {
    use ligrol_core::pack_path::PACK_EXTENSION;
    use ligrol_core::secrets::{PACK_ROOT_KEY_PREFIX, SERVICE, USER_DB_KEY, USER_SESSION_KEY};

    assert_eq!(PACK_EXTENSION, "nartik", "パックの拡張子");
    assert_eq!(SERVICE, "com.megablacklabel.ligrol", "keyring のサービス名");
    assert_eq!(USER_DB_KEY, "ligrol.db-key", "DB 鍵の keyring スロット");
    assert_eq!(
        USER_SESSION_KEY, "ligrol.session-key",
        "セッション鍵の keyring スロット"
    );
    assert_eq!(
        PACK_ROOT_KEY_PREFIX, "ligrol.pack-root-key:",
        "PRK の keyring スロット接頭辞"
    );
}

#[test]
fn drive_contract_is_ligrol() {
    use ligrol_core::drive::SYNC_FOLDER_NAME;
    use ligrol_core::pack_keys::KEY_BUNDLE_NAME;

    assert_eq!(SYNC_FOLDER_NAME, "ligrol", "Drive の同期フォルダ名");
    assert_eq!(KEY_BUNDLE_NAME, "ligrol-keys.json", "Drive の鍵バンドル名");
}
