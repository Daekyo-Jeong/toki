//! V4-19: 유저 펫 로더 — Tokisoft Studio가 export한 `~/.toki/pets/*.json`을
//! 읽어 외형 캐러셀/홈에 병합한다. 포맷 계약은 spec §5.7 (픽셀 `.o#@`,
//! 펫 = { id, name, states: { idle/hungry/focus…: {w,h,fps,frames} } }).
//! 깨진 파일은 통째로 스킵(경고 로그)하고 나머지는 살린다 — 파일 하나가
//! 전체 로드를 가라앉히지 않는다. 유저 펫은 해금 게이트 없음.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PetAnim {
    #[serde(default)]
    pub w: u32,
    #[serde(default)]
    pub h: u32,
    #[serde(default = "default_fps")]
    pub fps: f32,
    pub frames: Vec<Vec<String>>,
}

fn default_fps() -> f32 {
    3.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserPet {
    pub id: String,
    pub name: String,
    pub states: HashMap<String, PetAnim>,
}

/// 최소 무결성: idle 필수 + 프레임 비어있지 않음 + 픽셀 문자셋 `.o#@`.
/// (행 길이 불일치는 렌더러가 max-width로 흡수하므로 여기서 막지 않는다.)
fn validate(pet: &UserPet) -> Result<(), String> {
    if pet.id.trim().is_empty() || pet.name.trim().is_empty() {
        return Err("id/name 비어있음".into());
    }
    let idle = pet.states.get("idle").ok_or("states.idle 없음")?;
    if idle.frames.is_empty() {
        return Err("idle.frames 비어있음".into());
    }
    for (k, a) in &pet.states {
        for (i, f) in a.frames.iter().enumerate() {
            if f.is_empty() {
                return Err(format!("{k}#{i} 빈 프레임"));
            }
            for row in f {
                if !row.chars().all(|c| matches!(c, '.' | 'o' | '#' | '@')) {
                    return Err(format!("{k}#{i} 허용 외 문자 (픽셀은 .o#@ 만)"));
                }
            }
        }
    }
    Ok(())
}

pub fn load_user_pets() -> Vec<UserPet> {
    let Some(dir) = dirs::home_dir().map(|h| h.join(".toki").join("pets")) else {
        return Vec::new();
    };
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new(); // 디렉토리 없음 = 유저 펫 없음 (정상)
    };
    let mut files: Vec<_> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect();
    files.sort(); // 파일명 순 = 캐러셀 등장 순 (결정론)
    let mut out: Vec<UserPet> = Vec::new();
    for p in &files {
        let parsed = std::fs::read_to_string(p)
            .map_err(|e| e.to_string())
            .and_then(|s| serde_json::from_str::<UserPet>(&s).map_err(|e| e.to_string()));
        match parsed {
            Ok(pet) => match validate(&pet) {
                Ok(()) => out.push(pet),
                Err(e) => eprintln!("[pets] skip {}: {}", p.display(), e),
            },
            Err(e) => eprintln!("[pets] skip {}: {}", p.display(), e),
        }
    }
    // id 중복은 첫 파일이 이긴다 (파일명 순)
    let mut seen = std::collections::HashSet::new();
    out.retain(|p| seen.insert(p.id.clone()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // NOTE: 프레임 행이 "####"처럼 따옴표+해시 연속을 만들므로 raw string
    // 해시를 5개로 — 적게 쓰면 "## 등이 종결자로 오인돼 컴파일 에러.
    const SAMPLE: &str = r#####"{
        "$comment": "walrus-like fixture",
        "id": "walrus", "name": "엄니", "unlockLv": 1,
        "states": {
            "idle":  { "w": 4, "h": 2, "fps": 3, "frames": [[".@@.", "#oo#"], ["....", ".@@."]] },
            "focus": { "w": 4, "h": 2, "fps": 1, "frames": [[".@@.", "####"]] }
        }
    }"#####;

    #[test]
    fn parses_studio_export_shape() {
        // unknown keys($comment/unlockLv/w/h)는 무시·수용, states/frames 정상 파싱
        let pet: UserPet = serde_json::from_str(SAMPLE).expect("parse");
        assert_eq!(pet.id, "walrus");
        assert_eq!(pet.states["idle"].frames.len(), 2);
        assert!((pet.states["idle"].fps - 3.0).abs() < f32::EPSILON);
        assert!(validate(&pet).is_ok());
    }

    #[test]
    fn rejects_bad_charset_and_missing_idle() {
        let bad: UserPet = serde_json::from_str(
            r#"{"id":"x","name":"y","states":{"idle":{"frames":[["ab"]]}}}"#,
        )
        .unwrap();
        assert!(validate(&bad).is_err());
        let no_idle: UserPet =
            serde_json::from_str(r#"{"id":"x","name":"y","states":{"focus":{"frames":[["."]]}}}"#)
                .unwrap();
        assert!(validate(&no_idle).is_err());
    }
}
