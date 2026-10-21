# Changelog

## [0.4.0](https://github.com/noahdgrant/nightshift/compare/v0.3.0...v0.4.0) (2026-10-21)


### Features

* **cli:** a verification sees the previous verification and re-checks what failed ([#862](https://github.com/noahdgrant/nightshift/issues/862)) ([5d978f5](https://github.com/noahdgrant/nightshift/commit/5d978f5a192647939c7dca2442e7f1fd61561132))
* **cli:** keep each state's harness session and show it with ns log ([#873](https://github.com/noahdgrant/nightshift/issues/873)) ([31a8465](https://github.com/noahdgrant/nightshift/commit/31a8465c28063961b4741de50b039d80f41bca85))
* **cli:** merge only changes that touch no human_review path ([#872](https://github.com/noahdgrant/nightshift/issues/872)) ([9eee07e](https://github.com/noahdgrant/nightshift/commit/9eee07e0ba5e872269e5ff53d114393758eccdae))
* **cli:** ns assigns the unit and its change request to the account it runs as ([#868](https://github.com/noahdgrant/nightshift/issues/868)) ([87167c5](https://github.com/noahdgrant/nightshift/commit/87167c57fc58c437f5f3a6f934f27ced5dcbb064))
* **cli:** ns auto stops the night after units keep going wrong on the harness ([#861](https://github.com/noahdgrant/nightshift/issues/861)) ([c48e51b](https://github.com/noahdgrant/nightshift/commit/c48e51b9cc3886825f439374a3e521e02d4394e6))
* **cli:** ns eval review measures the review skill on known cases ([#874](https://github.com/noahdgrant/nightshift/issues/874)) ([d2bf77f](https://github.com/noahdgrant/nightshift/commit/d2bf77f71277adee4fcdf2aa9919ccaa8c17fe62))
* **cli:** the attempts a unit gets are a setting in nightshift.toml, three by default ([#878](https://github.com/noahdgrant/nightshift/issues/878)) ([692f39b](https://github.com/noahdgrant/nightshift/commit/692f39be43aad1de672ba0cba396b9b987335ead))


### Bug Fixes

* **cli:** resuming a unit stuck in a check with nothing changed goes back to build ([#863](https://github.com/noahdgrant/nightshift/issues/863)) ([adde79a](https://github.com/noahdgrant/nightshift/commit/adde79ab0849619d6f2fd5dd5f1d3c35b6e24e0a))

## [0.3.0](https://github.com/noahdgrant/nightshift/compare/v0.2.0...v0.3.0) (2026-10-21)


### Features

* **cli:** a review sees the findings of the previous review and says which are fixed ([#856](https://github.com/noahdgrant/nightshift/issues/856)) ([b4c3459](https://github.com/noahdgrant/nightshift/commit/b4c34594e3f7d8547825a18aeb924985ebe84d1c))
* **cli:** ns --version inside a pinned repository shows the version that runs there ([#848](https://github.com/noahdgrant/nightshift/issues/848)) ([a85fd95](https://github.com/noahdgrant/nightshift/commit/a85fd95fd2a2bd88ee87d6bc8fbf357b40d6b6c6))
* **cli:** ns status lists where each unit stands ([#854](https://github.com/noahdgrant/nightshift/issues/854)) ([9b8b219](https://github.com/noahdgrant/nightshift/commit/9b8b2191bf5b3d1d26dcd5809fab1336e62b8b3a))


### Bug Fixes

* **skills:** ns-verify fails a unit only for its requirements, not for a build claim it did not re-run ([#853](https://github.com/noahdgrant/nightshift/issues/853)) ([41401e1](https://github.com/noahdgrant/nightshift/commit/41401e12d1cc50c6a7c22c017d260bdb835e4455))

## [0.2.0](https://github.com/noahdgrant/nightshift/compare/v0.1.0...v0.2.0) (2026-10-21)


### Features

* **cli:** ns doctor checks the setup of a repository ([#840](https://github.com/noahdgrant/nightshift/issues/840)) ([42b5485](https://github.com/noahdgrant/nightshift/commit/42b548510a4207e07bcf5fb740163c439edd8a66))
* **cli:** ship gives the change request the unit's type, area and priority labels ([#844](https://github.com/noahdgrant/nightshift/issues/844)) ([49263aa](https://github.com/noahdgrant/nightshift/commit/49263aa6ff537c01ae333605ebd9a81e591387d1))


### Bug Fixes

* **cli:** clean up a finished unit once its change request has merged ([b527307](https://github.com/noahdgrant/nightshift/commit/b5273078b24bc46a492994d1ff1c3f7420747061))
* **cli:** nothing cleans up a unit after a human merges its change request ([#841](https://github.com/noahdgrant/nightshift/issues/841)) ([b527307](https://github.com/noahdgrant/nightshift/commit/b5273078b24bc46a492994d1ff1c3f7420747061))


### Documentation

* **repo:** add pin and sync to the glossary ([#837](https://github.com/noahdgrant/nightshift/issues/837)) ([4a4ac6d](https://github.com/noahdgrant/nightshift/commit/4a4ac6d1f255f3c561ddcb8a1eed66603d64d7b7))
* **repo:** the README says ns run stops before the first state, which it does not ([#845](https://github.com/noahdgrant/nightshift/issues/845)) ([c2c42bb](https://github.com/noahdgrant/nightshift/commit/c2c42bb57534b0d6df35164f67a577cbce602398))

## 0.1.0 (2026-10-19)


### Refactoring

* **repo:** replace the old nightshift with v2 and pin repositories to an ns version ([#830](https://github.com/noahdgrant/nightshift/issues/830)) ([4593c06](https://github.com/noahdgrant/nightshift/commit/4593c062fa47667fdfca4c97bb9fcd0b70db2b2f))
