# Sources and licenses

## Framework tooling

- REA 4.1.0: MIT; locked in `tools/package-lock.json`.
- MCP client 2.3.1: Apache-2.0; locked in `tools/package-lock.json`.
- Ghidra 12.1.4: existing local installation; its Apache-2.0 and bundled
  third-party notices apply. No Ghidra distribution is committed here.
- Temurin JDK 21.0.12.1+1: GPL-2.0 with Classpath Exception; downloaded from the
  official Adoptium release and checked against its published SHA-256.
- Local strace 7.2: signed Arch package for cache process traces; package files
  and license notices remain under ignored `.tools/`.

## Game assets

- Fonts embedded in the game by `src/ui.rs` and copied for the web page, all SIL Open Font
  License 1.1, from [google/fonts](https://github.com/google/fonts/tree/main/ofl):
  - [Chakra Petch](https://github.com/m4rc1e/Chakra-Petch) Medium, SemiBold and Bold by The Chakra
    Petch Project Authors, unmodified; license `assets/fonts/OFL-ChakraPetch.txt`.
  - [Big Shoulders Display](https://github.com/xotypeco/big_shoulders) by The Big Shoulders Project
    Authors: the ExtraBold (800) instance of the variable font, subset to Latin with fontTools;
    license `assets/fonts/OFL-BigShoulders.txt`.
  - [JetBrains Mono](https://github.com/JetBrains/JetBrainsMono) by The JetBrains Mono Project
    Authors: the Bold (700) instance of the variable font, subset to Latin with fontTools; license
    `assets/fonts/OFL-JetBrainsMono.txt`.

## Workflow reference

[2010 Rust Rewrite Mashup](https://github.com/chasmlol/2010-rust-rewrite-mashup)
at `f608f85e407ff1b7689d54a9aafdd16e95711ac4` (Apache-2.0), specifically
`AGENT.md`, `CONTEXT.md` and `docs/INDEX.md`, informed the private evidence,
handoff and short-document routing rules. Its runtime code was not copied.

Target programs and their assets remain the property of their respective owners.
This framework supplies no proprietary targets and grants no rights to them.
