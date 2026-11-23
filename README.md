# ARTEX Rust 재구축

**한국어** · [English](README.en.md) · [简体中文](README.zh-CN.md)

개발·검증 작업에는 [기술 스택과 작업 규칙](AGENTS.md)을 적용한다.

LLM 연결·기본 모델·일반 설정·에이전트·도구/MCP·스킬 구성은 작업 공간과 관계없이 앱 전체에서 공통으로 사용한다. 최근 사용 기록에 있는 공간의 모델은 첫 이전 때 자동으로 가져오며, 기록 밖의 공간은 직접 열어 이전한다. Bypass와 작업·대화 기록은 현재 작업 공간에 유지한다. [공통 설정 안내](docs/shared-application-settings.md)는 한국어로 제공한다.

사이트 분석 결과는 **작업 → 종합 보고서**에서 검사 내역·발견 문제·개선 우선순위·미검증 항목을 확인하고 Markdown 파일로 내보낼 수 있다. [보고서 사용 안내](docs/site-assessment-report.md)는 한국어로 제공한다.

GitHub: [froiibn/ARTEX-Desktop](https://github.com/froiibn/ARTEX-Desktop). 현재는 **공개 Windows 시험 배포 단계**다. Windows `v0.2.0-preview.5` 시험 빌드와 대응 소스를 Releases에서 제공하며 로그인 없이 다운로드할 수 있다. 원본 README의 별도 사용 제한은 [처리 방침](docs/license-policy.md)에 따라 파생 프로젝트의 추가 라이선스 조건으로 채택하지 않는다. 2026-10-15에 저장소를 공개로 전환했다. 이전 개발 이력과 `preview.1`의 GitHub 저장소는 제거했으며, 정리한 소스와 검증한 Windows 시험 배포를 GitHub에서 제공한다. [저장소 이력 안내](docs/repository-history.md)를 참고한다. 원본 ARTEX의 저자는 **Autumn-27**이다. [원작자·출처와 수정 사항](ARTEX_NOTICE.md)을 참고한다.

현재 개발 대상은 단일 Rust 프로세스의 네이티브 데스크톱 앱이다. Rust 네이티브 UI와 객체 모델·SQLite 저장, LLM·도구 실행 서비스가 같은 앱 안에서 동작한다. 기본 UI 실행에 Go, Node/npm, WebView, PostgreSQL 서버는 필요하지 않다. Python/Nmap 등 개별 외부 도구는 해당 기능을 사용할 때 별도로 필요하다.

이 프로젝트는 LLM 기반 보안 검증의 작업·에이전트·81개 등록 도구·자산·증거·보고를 로컬 작업 공간에서 관리하는 데스크톱 앱을 목표로 한다. [프로젝트 목적](docs/project-purpose.md)을 참고한다.

## 방어적 보안을 위한 공개 목적

보안 비전공자도 자신이 개발·운영하는 서비스에 모의침투를 수행해 취약점을 쉽게 분석하고, 발견한 문제를 이해하여 수정과 보안 강화로 이어갈 수 있도록 돕는 것이 목적이다. 취약점 발견부터 대응과 수정 후 재점검까지의 진입 장벽을 낮춰, 일반 사용자도 자신의 서비스를 더 안전하게 관리할 수 있도록 한다.

이 프로젝트는 사용자가 소유하거나 명시적으로 테스트 권한을 받은 서비스의 보안 점검과 방어 능력 향상을 위해 공개한다. 현재 Windows 시험 버전을 제공한다.

## 데스크톱으로 재구축한 이유

Docker 설치, 컨테이너 설정, 웹 서버와 PostgreSQL 구성 등은 비개발자와 일반 사용자에게 사용을 시작하기 어려운 진입 장벽이었다. 이 준비 과정을 줄여 **개발 지식이 없어도 일반적인 데스크톱 프로그램처럼 ARTEX를 사용할 수 있게 하는 것**이 재구축의 핵심 목적이다.

사용자는 실행 파일을 내려받아 압축을 풀고 앱을 실행한 뒤, 작업 공간과 LLM 모델을 설정해 작업을 시작한다. 원본의 에이전트 작업 흐름을 이어가면서 대화·도구 실행·승인·중지·결과 확인을 하나의 앱에 모았다.

- **시작 절차 단순화:** 기본 앱 실행에 Docker·웹 서버·PostgreSQL의 별도 설치와 운영이 필요하지 않다.
- **로컬 작업 관리:** 대화·작업·자산 관계와 실행 기록을 앱에 포함된 SQLite에 저장하고, 대형 증거는 파일로 보관하며, 에이전트가 공유하던 정보를 같은 작업 공간에서 연결한다.
- **실행 제어 통합:** 사용자 PC의 도구 실행, 승인, 진행 상태, 중지와 결과 확인을 데스크톱 화면에 모은다.
- **개발·배포 구조 통합:** UI와 앱 서비스를 Rust 단일 프로세스로 구성하고, Windows·macOS·Arch Linux를 목표로 같은 코드를 유지한다. 현재 시험 배포와 실제 GUI 검증을 완료한 대상은 Windows x64다.

작업 데이터는 로컬에 저장하며, LLM·원격 MCP·웹 요청을 사용할 때에는 해당 서비스·대상과 통신한다. 자세한 배경과 구성 비교는 [프로젝트 목적](docs/project-purpose.md)을 참고한다.

## 프롬프트의 영문 변환

LLM은 모델마다 지시를 해석하고 도구를 호출하는 방식이 달라, 같은 프롬프트를 사용해도 원본과 동일한 동작을 보장할 수 없다. 이러한 차이를 고려해 기본 프롬프트는 원본의 역할·맥락·작업 규칙과 도구 계약을 유지하면서 영문으로 변환했다. 실제 동작은 선택한 모델과 설정에 따라 확인한다. 대화와 보고서는 기본적으로 한국어로 응답하고, 사용자가 원하면 영어를 사용한다.

## 폴더

- `desktop/native`: 현재 Rust UI와 네이티브 검증·패키징 스크립트
- `desktop/native-core`: 앱 서비스
- `desktop/crates`: 저장·도메인·런타임·호스트·도구·데이터 이전 등 공통 Rust 라이브러리
- `desktop/contracts`: 빌드에 포함되는 고정 계약
- `docs`: 설계안·원본 비교 계약·81개 도구 대응·플랫폼 점검
- `scripts`: 원본 Go/Web 스키마와 동작을 비교하는 개발 도구
- `desktop/dist`: 배포 산출물
- `.github/workflows/native-rust.yml`: 현재 Rust 구현의 수동 실행 OS별 검증 워크플로
- `desktop/scripts`, `desktop/tests`, `scripts`: 도구·프롬프트·원본 계약의 비교 검증과 계약 유지 도구

이전 Tauri/React 앱과 별도 엔진 클라이언트는 현재 작업 트리에서 제거했다. [미사용 코드 정리 기록](docs/unused-code-cleanup.md)에 제거 범위와 복구 기준을 기록한다. 원본 Go/Web 저장소는 별도로 보존한다.

## 빌드 파일 다운로드

[Windows x64 시험 버전 v0.2.0-preview.5](https://github.com/froiibn/ARTEX-Desktop/releases/tag/v0.2.0-preview.5)의 Assets에서 `ARTEX-Desktop-v0.2.0-preview.5-windows-x64.zip`을 다운로드한다. 압축을 모두 풀고 `ARTEX-Windows-x64/ARTEX.exe`를 실행한다. 소스 빌드 도구 없이 기본 앱을 실행할 수 있다. 같은 릴리스에 해당 소스 ZIP과 SHA256SUMS.txt도 제공한다.

이 버전은 서명되지 않은 Windows 시험 빌드이며 설치 프로그램·자동 업데이트는 포함하지 않는다. macOS·Linux 실행 파일은 아직 제공하지 않는다. [첫 실행 안내](docs/download-and-run.md)를 참고한다.

## 간단 사용법

작업 공간은 대화와 점검 결과를 모아 두는 폴더다. 처음에는 아래 순서대로 시작한다.

1. **앱 실행:** 다운로드한 ZIP을 모두 풀고 `ARTEX.exe`를 실행한다.
2. **작업 공간 만들기:** 첫 화면에서 이름을 입력하고 **새 작업 공간 만들기**를 누른 뒤 저장할 폴더를 고른다. 다음에는 최근 작업 공간을 누르거나 **폴더에서 작업 공간 열기**로 같은 폴더를 연다.
3. **AI 연결:** 왼쪽 **환경 설정 → LLM 모델**에서 이름·API 형식·주소·모델을 입력한다. 키를 발급받은 서비스의 설정값을 사용한다. API 키 입력 → **비밀 값 저장** → **모델 설정 저장** → 목록에서 저장된 모델 선택 → **기본 모델로 설정** 순서로 진행한다. **저장된 모델 연결 시험**으로 연결을 확인한다. 키만 저장하면 설정이 끝난 것이 아니다.
4. **첫 대화:** **대화**에서 제목을 적고 **대화 만들기**를 누른다. 먼저 “안녕. 어떤 도움을 줄 수 있어?”를 보내 정상적으로 답하는지 확인한다.
5. **내 사이트 점검:** 아래 예시의 주소를 자신이 소유하거나 테스트 권한을 받은 사이트 주소로 바꿔 보낸다.

   > https://example.com 은 내가 운영하는 사이트야. 공개 페이지의 HTTPS와 보안 헤더를 가볍게 점검해줘. 대량 요청, 로그인 공격, 데이터 변경은 제외하고, 먼저 점검 계획을 설명해줘.

6. **진행·결과 확인:** 승인이 필요하면 **실행 승인**에서 내용을 읽고 결정한다. 답변과 도구 결과는 **대화**, 등록된 문제와 근거는 **발견 사항**·**트래픽·증거**, 진행 상태는 **에이전트 실행**에서 확인한다. 처음에는 상단 **Bypass**를 꺼 두면 도구 실행 승인을 직접 검토할 수 있다.
7. **멈추고 이어 하기:** 대화 응답은 대화 화면에서 중지한다. 작업 전체를 끝내려면 **작업**에서 해당 항목의 **작업 중지·정리**를 누르고 정리 결과를 확인한다. 저장된 작업 공간을 다시 열면 기록을 볼 수 있으며, 일시정지한 작업은 직접 재개한다.

“기본 모델이 없다”는 오류가 나오면 3번의 **기본 모델로 설정**까지 했는지 확인한다. Nmap·Python 같은 외부 도구가 없다는 오류는 해당 도구를 별도로 설치해야 한다는 뜻이다. 자세한 다운로드 안내는 [첫 실행 안내](docs/download-and-run.md)에 있다.

## 소스에서 빌드와 실행

Rust stable, Python 3.11 이상과 OS 네이티브 빌드 도구를 준비한다.

```powershell
# 이 저장소를 clone한 폴더에서 실행한다.
python desktop/native/scripts/build.py
& .\desktop\dist\ARTEX-Windows\ARTEX.exe
```

Windows 실행 파일은 항상 `desktop/dist/ARTEX-Windows/ARTEX.exe`에 생성·교체한다. macOS/Linux도 같은 빌드 명령을 사용하며 각각 `desktop/dist/ARTEX-macOS/artex-native`, `desktop/dist/ARTEX-Linux/artex-native`에 생성한다. Cargo 중간 파일은 `desktop/target` 한 곳에 모은다. 이 로컬 빌드는 Git에 포함하지 않으며 정식 배포 패키지 검증을 대신하지 않는다. [설정·실행 안내](desktop/native/README.md), [검증 현황](desktop/native/STATUS.md), [플랫폼 한계](docs/desktop-platform-audit.md)를 참고한다. macOS·Arch Linux 실기기 빌드/GUI 검증은 별도 과제다.

현재 위치에서 전체 네이티브 검증:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File desktop/native/scripts/verify.ps1 -FullRegression
```

## 원본과 Git

이 폴더는 별도 Git 저장소이며 공개 GitHub 저장소 `froiibn/ARTEX-Desktop`에 소스 기준점을 업로드한다. 기존 Go/Web 저장소와 번역 수정 이력은 `../original`에 보존했다. 실행 파일·실제/시험 작업 공간·API 키·생성 로그와 시험 보고서는 Git 업로드에서 제외한다. 로컬 전용 `desktop/dist`의 실행 파일은 clone에 포함되지 않으며 새 clone에서는 위 표준 빌드 명령으로 빌드한다.

일반 Cargo 빌드는 원본 저장소를 읽지 않는다. `scripts/validate-desktop-contracts.mjs` 등의 비교·생성 도구만 기본적으로 `../original`을 사용한다. 다른 원본 경로는 `ARTEX_ORIGINAL_ROOT` 환경 변수로 지정한다. 계약 생성 도구는 출력 파일을 다시 쓰므로 단순 검증에는 생성기를 실행하지 않는다. 원본 웹 개발 의존성은 원본 비교용 Node 도구에서만 사용한다.

이전 검증 결과의 절대 경로는 역사적 증거로 보존했다. `.resume` 임시 스크립트도 당시 경로를 사용할 수 있으므로 현재 작업에는 `desktop/native/scripts`를 사용한다. 사용자 데이터와 API 키는 이 소스 폴더와 별도로 관리된다.

SQLite 저장 전환과 기존 작업공간 이전·보존 방식은 [SQLite 저장 설계](docs/sqlite-storage.md)를 참고한다. 별도 DB 서버 설치는 필요하지 않다.

SQLite 전환은 현재 개발 소스에 적용된다. 기존 GitHub 릴리스 파일은 자동으로 갱신되지 않는다.
