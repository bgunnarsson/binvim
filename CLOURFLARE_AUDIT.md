# binvim security audit

Report recorded 2026-10-27. The completed `run-1` source audit was recovered from
`~/security-audit-skill/binvim/run-1/`, checked against the unchanged current commit,
and its findings and coverage validators rerun successfully. This document consolidates
that completed audit; it is not a new hunting pass or a Cloudflare service scan.
The six unresolved leads retain their original verdicts and runtime blockers.

Standard profile; whole-repository security-boundary review at `90a16e0f2506b9bd2d73f8c2aab49d9ab55e6221` (clean worktree at start). Source-only execution of the skill's sandboxed-source-and-local-only policy. No target code, builds, tests, fuzzers, installers or external services were run. No dependency fetching or live advisory-database scan was performed. This is a source review with explicit runtime validation limits, not a clean bill of health.

Audit workflow status: **complete**. No review budget was set; the recorded run used 32 review assignments across reconnaissance, hunting, coverage criticism and independent validation. Initial plan: four reconnaissance reviewers, six hunters, at least two critics, and two independent verification passes per surviving candidate; Windows and ctags follow-ups expanded the plan to eight hunters and five coverage critics. No compatible prior coverage-ledger/findings run existed. No prior confirmation was carried or suppressed.

## Result

0 confirmed vulnerabilities; **6 independently reviewed leads need validation**; 1 candidate claim rejected. The main unresolved boundaries concern automatic tool selection, document-to-terminal output, file access policy, installer module lookup, release credential handling and native parsing. Runtime reproduction was not attempted because the host lacks a verified sandbox enforcing all mandatory filesystem, aggregate resource and process-tree controls. Every lead below preserves that limitation; no severity is assigned to an unconfirmed lead.

## Confirmed findings

None. No runtime security effect was demonstrated.

## Needs validation

| Lead | Source | Decisive next observation |
| --- | --- | --- |
| Document control characters reach the terminal command channel | [src/render.rs:6736](src/render.rs#L6736) | The terminal emulator and its OSC 52 clipboard policy are external runtime facts. Source alone does not demonstrate an accepted clipboard mutation or establish that every supported terminal permits it. |
| Bash highlighting passes Unicode to narrow character classification | [src/lang.rs:904](src/lang.rs#L904) | No bounded native check establishes that the supported compiler/C-library implementation performs an out-of-range read for this input. A range-checking implementation or optimization can prevent that result. The repository's historical CI comment neither identifies an exact failing input nor establishes that these classifier calls caused the fault. |
| Windows ancestor-tool discovery omits the cross-user ownership boundary | [src/lsp/client.rs:96](src/lsp/client.rs#L96) | The actual supported Rust/Windows executable resolver and cross-user ACL setup require a contained runtime check. Ancestor discovery requires an extensionless biome file; biome.exe alone does not satisfy its is_file check. Locally installed Rust Windows process source prefers an appended .exe sibling for an explicit extensionless path, but the shipped Windows toolchain/runtime was not exercised. |
| Release-token probe places the crates.io credential in process arguments | [scripts/release.sh:488](scripts/release.sh#L488) | Cross-user command-line visibility and whether the installed curl masks this Authorization header argument have not been established on a supported release workstation. A meaningful disclosure requires a different lower-trust OS principal that cannot already read the original credential source, can observe this child during the available interval, and obtains a token that still grants protected registry authority. These host policy and token-scope conditions are not repository-established. |
| Python tool installation may resolve modules from the inherited workspace | [src/install.rs:1421](src/install.rs#L1421) | The selected Python runtime's effective module-search policy is not established from repository source. Safe-path settings, isolated interpreter packaging or other startup policy may suppress working-directory lookup. The candidate requires a lower-trust module source in the inherited process working directory or import path and an approved PythonModule Install or Update choice. |
| Atomic saving can discard a file-specific access-control list | [src/paths.rs:541](src/paths.rs#L541) | No bounded two-principal filesystem check establishes the original and replacement effective ACL decisions on a supported OS. The original file must restrict the second user beyond basic permissions, the parent directories must permit traversal, the saving owner must be able to replace the inode, inherited/default ACLs must not preserve the restriction, and the in-place fallbacks must not apply. Source alone does not establish those environmental conditions. |

Complete source traces, exact blockers and bounded validation plans appear below. These are source-grounded hypotheses, not confirmed vulnerabilities.

## Rejected claim

The root-owned group-writable tool claim was rejected: `src/paths.rs:1209-1212` explicitly tests that policy, and no excluded lower-trust group principal was established. The record is retained in the audit run’s `findings.json` to avoid re-reporting the same unsupported premise.

## Concrete non-security correctness issue

With `preview_workspace_edits=true`, a second server edit arriving while a preview is open can fall through to immediate application: `open_server_apply_edit_preview` returns false for busy state, and its caller interprets false as apply directly. See [app/lsp_glue.rs:73](src/app/lsp_glue.rs#L73) and [app/rename_preview.rs:85](src/app/rename_preview.rs#L85). Distinguish disabled preview from rejected/busy preview and reply `applied:false` for the latter. This is not classified as a security finding because the installed server already has the user’s filesystem authority and no lower-trust triggering path was established.

## Hardening and positive controls

- Private cache roots are screened against effective UID on Unix, and undo/recovery directories are narrowed to 0700. Atomic file creation is exclusive; save paths preserve basic ownership and permissions and refuse selected data-loss conditions. ACL preservation is a separate unresolved lead.
- Notebook output text strips controls; the PTY emulator drops OSC/DCS host actions. Those controls do not establish safety of every document-rendering path.
- DAP reverse requests, including runInTerminal, are rejected; LSP responses are keyed by client and request ID. Kernel bridge startup uses a neutral working directory, and output IDs are correlated with execution requests.
- Protocol body/queue caps, image decode/cache limits and complete DAP response-generation checks would improve robustness. No separate shared-service availability violation was established.
- Notebook export IDs could be made filename-safe. The hash-prefixed filename and absence of attacker-created intermediate directories prevented a complete document-only traversal claim.
- Installer signature verification and immutable CI input pins would improve supply-chain assurance; missing signatures or floating tags alone were not promoted to vulnerabilities.

## Coverage and limitations

The ledger contains **50 coverage units**: 21 candidate, 29 covered. Candidate units may share a root cause; there is one final record per fingerprint. The candidate state retains traceability to reviewed leads, not unfinished independent review. The last post-wave critic and the distinct final-clean critic accepted no remaining source-backed unit or reassignment.

Reviewed groups: private persistence and saves; documents/notebooks/terminal output; workspace discovery and automatic execution; LSP/DAP/kernel/PTY clients; installation, CI and release; native parser integration and vendored Markdown; Windows tool-discovery and ctags-navigation follow-ups. No deferred or out-of-scope ledger units remain. This does not claim every source line or dependency implementation was exhaustively audited.

Important limits: no runtime/security fixtures; no Windows ACL/process fixture; no complete third-party grammar or image-decoder audit; no sanitizer/fuzzer execution; no live dependency vulnerability database check; no hosted branch/tag protection, runner, registry or signing-policy inspection; no terminal emulator/clipboard-policy verification. The pinned Bash source received targeted read-only cache inspection, which is explicitly distinguished from repository-contained evidence. These limits remain even though the source-review workflow is complete.

The structured findings and coverage ledger passed the skill’s validators. Validation proves record consistency, not exploitability. This report changes documentation only; application source is unchanged.

## Detailed leads requiring validation

These records received independent source review and final record verification. No target code ran and no runtime security effect is confirmed. Use only the contained dummy-data plans below; do not probe live services.

### Document control characters reach the terminal command channel

Fingerprint: `binvim/render/document-control-bytes-terminal-channel`. Verdict: **needs validation**, no severity assigned.

A lower-trust document author can place ESC sequences in text that binvim passes to terminal output without displaying the controls literally. An unhighlighted, undecorated and fully visible text run can preserve adjacent characters without interleaved style commands. On a terminal permitting OSC 52 clipboard writes, viewing such a document could replace the developer clipboard without a copy action. The source path is established; no terminal effect, execution or disclosure was demonstrated.

Source hypothesis: The file decoder preserves ESC/BEL; draw_line_with_selection removes trailing LF/CR but otherwise emits characters through crossterm Print without escaping terminal controls. Width clipping and style selection are not control-byte filtering. Whole-string Markdown summary and Copilot ghost-preview sinks have the same output-encoding omission. Notebook output display_line and DAP ANSI parsing apply separate controls that do not protect these paths. The ghost sink is a sibling encoding observation, not evidence that an attacker can induce a particular provider response.

#### Source trace

1. [src/buffer.rs:277](src/buffer.rs#L277) — `Buffer::from_path`: Reads document bytes and decodes them into text; from_path then stores the result in the Rope.
2. [src/buffer.rs:48](src/buffer.rs#L48) — `decode_file_text`: Removes a leading BOM and folds CRLF only; ESC and BEL remain document characters.
3. [src/render.rs:6170](src/render.rs#L6170) — `draw_line_with_selection`: Collects the line characters, measuring controls with the width fallback rather than escaping them.
4. [src/render.rs:6638](src/render.rs#L6638) — `draw_line_with_selection`: Unhighlighted consecutive characters reuse the already selected foreground, so terminal sequences can remain contiguous rather than necessarily being interrupted by style commands.
5. [src/render.rs:6736](src/render.rs#L6736) — `draw_line_with_selection`: Prints each remaining character directly to terminal output without a control-character guard.

#### Evidence

- [src/buffer.rs:53](src/buffer.rs#L53) — The decoder only replaces CRLF; no terminal control escaping occurs.
- [src/render.rs:6773](src/render.rs#L6773) — plain_fg_on is preserved across ordinary unstyled characters.
- [src/markdown_render.rs:593](src/markdown_render.rs#L593) — Summary title extraction removes HTML tags but preserves remaining control characters.
- [src/render.rs:6361](src/render.rs#L6361) — A Markdown summary replacement is printed as one string after width clipping.
- [src/render.rs:6873](src/render.rs#L6873) — Copilot ghost text is width-truncated and printed before acceptance, with no control filtering.
- [src/notebook.rs:1182](src/notebook.rs#L1182) — The separate notebook output display path drops ESC and control characters; it does not protect raw document or ghost rendering.
- [src/render.rs:6120](src/render.rs#L6120) — The renderer obtains the Rope line and removes only trailing LF and CR; ESC/BEL are not removed before the character walk.
- [src/render.rs:26](src/render.rs#L26) — char_width treats control characters without a Unicode width as one display cell, and visible_slice copies fully visible characters at line 97.
- [src/ansi.rs:60](src/ansi.rs#L60) — The separate DAP SGR parser drops CSI/OSC sequences; draw_line_with_selection does not use this parser.

#### Exact blockers

- No target-controlled renderer fixture was executed because the required OS sandbox has not been established: read-only target/tools, denied host credentials/sockets, scratch-only writes, and aggregate memory/process/disk plus process-tree wall-clock enforcement are unverified.
- The terminal emulator and its OSC 52 clipboard policy are external runtime facts. Source alone does not demonstrate an accepted clipboard mutation or establish that every supported terminal permits it.

#### Bounded validation plan

**Local:** Inside the fully constrained offline sandbox, use a small renderer harness with a dummy .txt buffer containing the escaped fixture \x1b]52;c;YXVkaXQ=\x07 (the base64 content is only the harmless marker audit). Render an unhighlighted line with no selection, search/yank/document highlights, diagnostic underline, inlay hints, multicursors or modal dimming, view_left zero, and enough width for the complete sequence. Capture one frame to a bounded byte sink and verify whether the exact sequence survives; include an ordinary printable-text control. Feed only that frame into an isolated terminal emulator with a dummy clipboard and explicitly documented OSC 52 policy; determine whether the dummy clipboard changes without a copy action. Separately exercise a visible Normal-mode Markdown <summary> containing the same marker. A synthetic ghost-preview fixture may check the sibling encoding sink but does not establish that an attacker controls provider output. Do not use the host clipboard, terminal, real credentials or external network; stop at one dummy-clipboard observation.

### Bash highlighting passes Unicode to narrow character classification

Fingerprint: `lang/bash-scanner/unrestricted-unicode-ctype`. Verdict: **needs validation**, no severity assigned.

A lower-trust file author can supply a small shell, EditorConfig or ignore file that binvim automatically parses with the pinned native Bash grammar. Production preserves Unicode, and the pinned scanner contains narrow isdigit calls on decoded lexer lookahead in brace-range scanning. Whether a reachable non-byte value causes an invalid native read depends on the actual parser state, compiler and C library. No invalid read, crash, disclosure or code execution was observed in this audit; the historical CI comment is not a reproduction or proof of this root cause.

Source hypothesis: The cached tree-sitter-bash 0.25.1 scanner calls isdigit(lexer->lookahead) after consuming an opening brace and again after the range separator, without an ASCII/unsigned-character-range guard. The lexer field is int32_t and receives decoded code points. Binvim passes the full Unicode source to that native parser. This is a source-grounded hypothesis of an out-of-range classifier read on an affected implementation; a compiler or C library that implements classification with a safe range check may prevent the claimed memory effect. The repository's Markdown scanner uses explicit ASCII digit comparisons, but does not protect the Bash-backed grammar.

#### Source trace

1. [src/buffer.rs:277](src/buffer.rs#L277) — `Buffer::from_path`: Reads lower-trust document bytes; UTF-8 text is decoded at line 281 and stored in the Rope at line 287. Valid non-ASCII scalars are preserved.
2. [src/lang.rs:118](src/lang.rs#L118) — `Lang::detect`: Shell file extensions select Bash; EditorConfig and ignore filenames also select their Bash-backed variants at lines 167 and 173.
3. [src/app/view.rs:1134](src/app/view.rs#L1134) — `App::ensure_highlights`: Non-large files proceed automatically to compute_highlights at line 1149; the size gate does not reject small multibyte inputs.
4. [src/lang.rs:333](src/lang.rs#L333) — `Lang::ts_language`: Bash uses tree_sitter_bash::LANGUAGE; EditorConfig and GitIgnore share it at line 360.
5. [src/lang.rs:945](src/lang.rs#L945) — `compute_byte_colors_until`: The source string is sent to parse_bounded without filtering Unicode.
6. [src/lang.rs:904](src/lang.rs#L904) — `parse_bounded`: The native parser receives the original UTF-8 bytes. Its progress deadline bounds cooperative parse work but does not validate native scanner character-classifier arguments.

#### Evidence

- [Cargo.lock:1265](Cargo.lock#L1265) — Pins tree-sitter-bash 0.25.1 and its registry checksum; cached source inspection of that exact version is supplemental, not repository-contained evidence.
- [src/lang.rs:2719](src/lang.rs#L2719) — Records a historical multibyte Bash scanner concern and nondeterministic native x86_64 fault. No captured reproducer or proof linking that fault to the narrow isdigit calls is present; the comment is contextual evidence only.
- [src/lang.rs:2733](src/lang.rs#L2733) — BASH_FUZZ_ALPHABET restricts input to printable ASCII plus tab/newline; usages at 2769,2772,2773 omit production multibyte inputs.
- [src/lang.rs:901](src/lang.rs#L901) — parse_bounded retains the complete source.as_bytes slice for the native parser callback.
- [vendor/tree-sitter-markdown/src/scanner.c:735](vendor/tree-sitter-markdown/src/scanner.c#L735) — The repository implements an ASCII digit range guard for the analogous Markdown classifier problem, providing a source-visible baseline rather than a Bash fix.
- [src/buffer.rs:48](src/buffer.rs#L48) — decode_file_text removes a BOM and folds CRLF, leaving valid Unicode content available for highlighting.

#### Exact blockers

- No approved OS-enforced sandbox provides all required no-network, read-only source/toolchain, scratch-only writes, empty environment, aggregate memory/process/disk and process-tree wall-clock controls; no target code or parser fixture was run.
- Read-only inspection of cached tree-sitter-bash 0.25.1 finds narrow isdigit calls in src/scanner.c:1158 and 1172 and an int32_t lookahead field in src/tree_sitter/parser.h:49. This dependency source is outside the repository. The brace token must be valid outside error recovery for those calls to execute; exact runtime reachability has not been demonstrated.
- No bounded native check establishes that the supported compiler/C-library implementation performs an out-of-range read for this input. A range-checking implementation or optimization can prevent that result. The repository's historical CI comment neither identifies an exact failing input nor establishes that these classifier calls caused the fault.

#### Bounded validation plan

**Local:** In a fully contained offline fixture with the pinned dependency, use the production Bash highlighter on a tiny data string such as echo {\U0001F600..1} (interpreting the escape as one Unicode scalar), never as a shell command. Instrument the scanner to record whether BRACE_START is valid outside error recovery and the actual isdigit argument; test the second range endpoint separately. Inspect the compiled classifier and use an appropriate sanitizer or checked-classifier harness to distinguish a contract violation from an actual invalid native read. If the implementation range-checks safely, do not call the precondition mismatch a demonstrated memory-safety effect. Record the exact compiler, C library, flags, scalar and minimum result; repeat through EditorConfig/GitIgnore language selection. Enforce no network, empty allowlisted environment, read-only source/tools, scratch-only writes and strict CPU/memory/process/file-size/disk/process-tree time limits. Stop at one minimum observation; do not use heap shaping, disclosure or execution payloads. Compare a scratch-only scanner change using explicit ASCII digit ranges if a failing implementation is established.

### Windows ancestor-tool discovery omits the cross-user ownership boundary

Fingerprint: `paths/others_can_plant/windows-ancestor-tool-trust`. Verdict: **needs validation**, no severity assigned.

A different Windows user with write access to an ancestor node_modules/.bin subtree may supply a tool for a victim-owned child project that the attacker cannot otherwise modify. When a fresh editor opens a small JSON file with no nearer Biome, discovery accepts an existing extensionless biome file and passes that explicit path to process creation without owner/ACL screening. An extensionless sentinel plus a biome.exe sibling is a concrete fixture for the runtime resolver. No cross-user execution effect has been observed.

Source hypothesis: others_can_plant returns false unconditionally on non-Unix. Both ancestor node_modules executable selection and LSP workspace-root screening rely on that helper. The candidate therefore bypasses the cross-user screening implemented on Unix; it is distinct from intentionally using a tool inside a same-user project. Formatter discovery shares the same unchecked ancestor path.

#### Source trace

1. [src/lsp/specs.rs:800](src/lsp/specs.rs#L800) — `find_node_modules_bin`: During automatic JSON server selection, walk ancestors and read the existence of node_modules/.bin/biome, including a subtree writable by a different OS user. Canonicalizing the starting directory does not establish ownership of the selected file.
2. [src/paths.rs:241](src/paths.rs#L241) — `others_can_plant[not(unix)]`: The Windows implementation always returns false, so the is_file-and-trust decision at lsp/specs.rs:806 accepts the extensionless candidate without owner or ACL checks.
3. [src/lsp/specs.rs:193](src/lsp/specs.rs#L193) — `primary_spec_for_path`: The discovered ancestor path becomes the Biome ServerSpec command candidate, ahead of the PATH fallback.
4. [src/lsp/manager.rs:270](src/lsp/manager.rs#L270) — `LspManager::ensure_for_path`: Before a new client spawn, the workspace-root screen uses the same always-false helper. An existing client would instead be reused, so the proposed fixture requires a fresh manager with no Biome client.
5. [src/lsp/specs.rs:1090](src/lsp/specs.rs#L1090) — `resolve_command`: The explicit discovered path passes another is_file check and is returned unchanged; no final Windows executable owner or ACL check is added.
6. [src/lsp/client.rs:96](src/lsp/client.rs#L96) — `LspClient::spawn_spec`: Command::new receives the accepted explicit path and spawn is called at line 104 with inherited editor authority, subject to the Windows executable resolver.

#### Evidence

- [src/lsp/manager.rs:270](src/lsp/manager.rs#L270) — Independent workspace-root screening uses the same always-false helper; it cannot restore the missing Windows owner/ACL boundary.
- [src/paths.rs:199](src/paths.rs#L199) — PATH lookup synthesizes .exe/.cmd/.bat candidates, but this does not apply to the exact ancestor node_modules path. A fixture must meet the extensionless discovery precondition.
- [src/format.rs:361](src/format.rs#L361) — Biome formatting on save reuses find_node_modules_bin and spawns the selected path at lines 371-377.
- [src/format.rs:229](src/format.rs#L229) — Prettier formatting uses the same ancestor discovery and passes the path to run_stdin_pipe at line 237, whose spawn is at lines 77-83.
- [src/app/save.rs:75](src/app/save.rs#L75) — An ordinary save invokes format_buffer, making formatter selection a sibling activation path.
- [docs/windows.md:5](docs/windows.md#L5) — The repository explicitly supports x86_64-pc-windows-msvc; this is not merely an unreachable conditional compilation branch.
- [src/app/buffers.rs:307](src/app/buffers.rs#L307) — Opening a new buffer invokes lsp_attach_active automatically; simply reopening an already-open buffer takes a different branch.
- [src/app/lsp_glue.rs:953](src/app/lsp_glue.rs#L953) — A named non-large buffer with LSP text reaches ensure_for_path, without an execution confirmation for the discovered ancestor tool.
- [src/lsp/specs.rs:189](src/lsp/specs.rs#L189) — JSON/JSONC chooses the nearest discovered ancestor Biome before the global PATH fallback.

#### Exact blockers

- No approved Windows execution environment provides all required no-network, empty-environment, read-only target/toolchain, scratch-only-write, aggregate memory/process/disk and whole-process-tree wall-clock controls. No target code or Windows fixture was executed.
- The actual supported Rust/Windows executable resolver and cross-user ACL setup require a contained runtime check. Ancestor discovery requires an extensionless biome file; biome.exe alone does not satisfy its is_file check. Locally installed Rust Windows process source prefers an appended .exe sibling for an explicit extensionless path, but the shipped Windows toolchain/runtime was not exercised.

#### Bounded validation plan

**Local:** Use a fully contained Windows fixture with two dummy standard-user accounts, no network or real credentials, an empty allowlisted environment, read-only source/toolchain and strict aggregate resource/process-tree limits. Under assigned scratch, grant attacker write access only to a shared ancestor node_modules/.bin subtree; keep a victim-owned child project inaccessible for attacker read, write and deletion, while allowing victim traversal/read/execute of the tool subtree. Place a non-executable extensionless biome sentinel and a harmless biome.exe that only writes one fixed non-secret marker to victim scratch. Use a tiny JSON file, no nearer Biome, and a fresh LSP manager with no existing Biome client. Trace the production discovery/spawn path and record the exact selected executable and whether the marker appears. Stop after that observation. Run a control without the extensionless sentinel and a control without the ancestor candidate, using a harmless trusted PATH tool if needed; do not assume revoking attacker write permission removes previously planted bytes. Separately inspect the shared Biome/Prettier formatter resolution seam with dummy programs only. Record safe dummy identities, relevant ACLs, exact executable path and Rust/Windows toolchain identity. Do not use a real shared directory or user account.

### Release-token probe places the crates.io credential in process arguments

Fingerprint: `scripts/release.sh:probe_crates_token:secret-in-argv`. Verdict: **needs validation**, no severity assigned.

The release script reads a configured crates.io token from an environment variable or Cargo credentials file and expands it into a curl argument. A different local OS principal that cannot read the original credential but can inspect this child process command line may obtain the token during the probe. The source does not echo the token; that does not establish argument confidentiality. No actual credential, ambient process, cross-user disclosure or token capability was inspected or demonstrated.

Source hypothesis: probe_crates_token passes the complete Authorization header as one curl -H argument. This copies credential bytes from the configured credential source into child-process arguments without a repository-controlled masking or private-input mechanism. Whether a different OS principal can observe those bytes, including before any curl-side masking, depends on the platform and installed curl implementation.

#### Source trace

1. [scripts/release.sh:453](scripts/release.sh#L453) — `probe_crates_token`: Read CARGO_REGISTRY_TOKEN into the local token variable or, if absent, extract the first matching token assignment from the configured Cargo credentials file at line 457; no actual credential is inspected by this audit.
2. [scripts/release.sh:488](scripts/release.sh#L488) — `probe_crates_token`: Expand the complete token into one child argv element using -H "Authorization: ${token}" at line 489; stdout captures the response and stderr is suppressed, neither of which removes the argument supplied at process creation.

#### Evidence

- [scripts/release.sh:425](scripts/release.sh#L425) — The script documents the credential sources for subsequent publishing; actual credential file ACLs and token privileges are not established by this comment.
- [scripts/release.sh:448](scripts/release.sh#L448) — Comment promises the token is not echoed; no source control prevents exposure through child arguments.
- [scripts/release.sh:488](scripts/release.sh#L488) — curl receives the token in argv, has a 15-second timeout, and stderr is suppressed; suppression does not alter argv.
- [scripts/release.sh:577](scripts/release.sh#L577) — The release later invokes cargo publish --locked; token scope remains an owner/runtime fact.
- [scripts/release.sh:513](scripts/release.sh#L513) — Normal release preflight invokes probe_crates_token, so the argv construction is reachable before local verification and cargo publish; no extra token-probe opt-in is required.
- [scripts/release.sh:459](scripts/release.sh#L459) — Empty token or missing curl returns before the sink; the candidate requires a nonempty token and an available curl executable.

#### Exact blockers

- Mandatory target-execution sandbox is not established: no verified aggregate memory/process/disk limits, scratch-only writes with denied host credential access, and whole-process-tree wall-clock cleanup. No target code or runtime fixture was executed.
- Cross-user command-line visibility and whether the installed curl masks this Authorization header argument have not been established on a supported release workstation. A meaningful disclosure requires a different lower-trust OS principal that cannot already read the original credential source, can observe this child during the available interval, and obtains a token that still grants protected registry authority. These host policy and token-scope conditions are not repository-established.

#### Bounded validation plan

**Local:** After all mandatory containment controls are established, use two dummy OS users, an empty allowlisted environment, read-only tools/source, scratch-only writes and strict aggregate resource and process-tree limits. Supply a non-secret dummy token through a scratch credential input inaccessible to the second user. Exercise only the narrow curl probe argument construction against an isolated-loopback mock, with external networking disabled and a short bounded hold. From the second dummy principal inspect only the known fixture child PID through the platform process API and determine whether the dummy Authorization marker is visible, accounting for any curl-side masking. Do not inspect ambient processes, invoke the complete release script, use real credentials, publish or contact crates.io. Stop at one marker observation. Compare with a private descriptor or stdin configuration that keeps header contents out of argv.

**Deployment:** The owner can inspect the release workstation process-inspection policy, account isolation and intended token scope without revealing the token. Establish whether a different lower-trust OS user can see the releaser child arguments while being unable to read the original credential source. Do not run the real token probe or inspect real credential values for this check.

### Python tool installation may resolve modules from the inherited workspace

Fingerprint: `src/install.rs:python_module_steps:workspace-module-shadowing`. Verdict: **needs validation**, no severity assigned.

A lower-trust repository author can supply Python module files in the process working directory. After the developer approves a catalogued debugpy or ipykernel installation or update, binvim constructs Python module-invocation commands without selecting a neutral directory or explicit import-isolation flags. Whether those commands resolve repository content instead of the intended installer modules depends on the selected Python runtime and import policy. No runtime execution or security effect was observed.

Source hypothesis: python_module_steps emits -m venv and -m pip without isolated or safe-path flags. argv_command sets only the executable and arguments; run_plan invokes the resulting commands without current_dir or environment narrowing. The separate python_has_module discovery probe removes PYTHONPATH and filters its own sys.path, but those changes apply only to the probe process and are not applied to the installation commands. User confirmation authorizes the displayed package installation; it does not establish the provenance of modules subsequently resolved by Python.

#### Source trace

1. [src/install.rs:879](src/install.rs#L879) — `argv_command`: The command builder leaves the process working directory and import-related environment inherited, allowing workspace contents to remain potential inputs to Python module resolution.
2. [src/install.rs:853](src/install.rs#L853) — `python_module_steps`: Missing-venv setup selects venv through -m; the ready-venv and fallback commands select pip through -m at lines 857 and 859. No explicit isolated or safe-path flag is added.
3. [src/install.rs:1399](src/install.rs#L1399) — `run_plan`: For an approved Install or Update choice, constructs the displayed package command through build_command or upgrade_command without adding directory or import-environment restrictions.
4. [src/install.rs:1421](src/install.rs#L1421) — `run_plan`: Calls status on the setup command, then on the package command at line 1425 if setup did not fail; inherited context is unchanged at this launch point.

#### Evidence

- [src/install.rs:454](src/install.rs#L454) — The fixed tool catalogue selects PythonModule for debugpy and, at line 459, ipykernel. Package names are not repository-selected.
- [src/install.rs:775](src/install.rs#L775) — python_has_module filters the probe's sys.path and removes PYTHONPATH at line 781; these safeguards are local to that separate Command and do not mutate the parent environment.
- [src/install.rs:879](src/install.rs#L879) — argv_command calls only Command::new and args. It sets neither current_dir nor Python import-related environment controls.
- [src/install.rs:203](src/install.rs#L203) — PythonModule installation uses python_module_command; the update branch at line 279 uses the same builder with upgrade enabled.
- [src/install.rs:318](src/install.rs#L318) — setup_command delegates to python_module_setup and configures inherited stdio only, without selecting a neutral directory or narrowing import environment.
- [src/app/installer.rs:418](src/app/installer.rs#L418) — The in-editor Plan stage requires y/Y before run_install_plan; this is an explicit installation action, not automatic execution from merely opening a file.
- [src/app/installer.rs:706](src/app/installer.rs#L706) — The in-editor runner passes the selected plan to run_plan without adding a directory or environment restriction.
- [src/bin/binvim-install.rs:460](src/bin/binvim-install.rs#L460) — The standalone flow displays the plan, requires confirmation at line 462 and invokes run_plan at line 467 without selecting a neutral directory.
- [src/bin/binvim-install.rs:365](src/bin/binvim-install.rs#L365) — Standalone confirmation defaults to no and accepts y/yes; this limits the candidate to an intentionally approved package operation.

#### Exact blockers

- Mandatory target-execution isolation is not established: no verified aggregate memory/process/disk limits, scratch-only writes with denied host credential access, and whole-process-tree wall-clock cleanup. No target code, installer, Python probe or runtime fixture was executed.
- The selected Python runtime's effective module-search policy is not established from repository source. Safe-path settings, isolated interpreter packaging or other startup policy may suppress working-directory lookup. The candidate requires a lower-trust module source in the inherited process working directory or import path and an approved PythonModule Install or Update choice.

#### Bounded validation plan

**Deployment:** The owner can inspect the selected Python interpreter's configured import-isolation policy and the installer launch context, including whether the working directory contains lower-trust repository content and whether safe-path or equivalent settings exclude it. These configuration observations can refute the prerequisite. The actual module-resolution result remains unobserved; this narrowed source-only review supplies no new reproduction input or execution procedure.

### Atomic saving can discard a file-specific access-control list

Fingerprint: `src/paths.rs:write_atomic_with:acl-loss-on-replace`. Verdict: **needs validation**, no severity assigned.

On an ACL-capable filesystem, a different local user denied access by a file-specific ACL may gain access after the owner saves the file through the ordinary replacement path. That path creates a new inode and restores basic permissions and, on Unix, owner/group, but does not copy the original ACL. The hypothesis requires a restrictive original ACL, broader effective permissions on the replacement, traversable parent directories and none of the in-place fallback conditions. No cross-user access result was demonstrated.

Source hypothesis: write_atomic_with creates a sibling inode, applies Metadata permissions and Unix owner/group, writes content and renames it over the target without an ACL-copy step. The source explicitly documents that ACLs and extended attributes are not carried over. Exclusive creation prevents replacement of the temporary leaf, and the original write-open check preserves the saving user’s write restriction; neither preserves ACL restrictions on a different reader. Default or inherited directory ACLs can retain the restriction on the new inode, so actual access widening is platform and directory-policy dependent.

#### Source trace

1. [src/app/save.rs:55](src/app/save.rs#L55) — `App::save_active`: An owner saves an existing ACL-bearing file; changed-on-disk and lossy-input checks run, then save_with_outputs is called at lines 105-106. The affected protected resource is the existing file and its reader policy.
2. [src/buffer.rs:378](src/buffer.rs#L378) — `Buffer::save_with_outputs`: Serialized buffer bytes are handed to the shared atomic writer.
3. [src/paths.rs:512](src/paths.rs#L512) — `write_atomic_with`: Outside hard-link/temp-directory/permission fallbacks, create a new sibling inode and copy owner/group via take_owner.
4. [src/paths.rs:541](src/paths.rs#L541) — `write_atomic_with`: After set_permissions at line 531 and writing/syncing content, rename replaces the original with the new inode. No original ACL-copy step occurs; rename failure instead selects in-place writing.

#### Evidence

- [src/paths.rs:419](src/paths.rs#L419) — The atomic-writer contract explicitly states ACLs and extended attributes are not carried over.
- [src/paths.rs:495](src/paths.rs#L495) — Multiple hard links select in-place writing; an existing file under a configured temporary directory does so at lines 509-510. PermissionDenied creating the sibling, failed owner transfer or failed rename also select in-place writing.
- [src/paths.rs:504](src/paths.rs#L504) — Opening the original for writing is the strongest access check, but establishes the saving principal can write; it does not preserve restrictions on other readers.
- [src/paths.rs:692](src/paths.rs#L692) — Exclusive creation copies mode bits only on Unix.
- [src/paths.rs:702](src/paths.rs#L702) — Unix take_owner checks or changes only uid/gid via fchown. Its non-Unix implementation at line 714 simply returns true; this review does not infer Windows ACL semantics.
- [src/paths.rs:531](src/paths.rs#L531) — The final metadata restoration uses set_permissions, then the replacement is renamed at line 541.
- [src/paths.rs:666](src/paths.rs#L666) — The replacement is created beside the original file. Its ACL may therefore be derived from the parent directory at creation, rather than copied from the original file.
- [src/paths.rs:1110](src/paths.rs#L1110) — Existing Unix tests assert owner/group and basic mode preservation (mode test at line 1123), but do not establish ACL preservation or effective access for another principal.

#### Exact blockers

- No approved execution sandbox has been established with read-only target/toolchain, scratch-only writes, no external network, empty allowlisted environment, aggregate memory/process/disk quotas and whole-process-tree time limits; target execution is prohibited in this run.
- No bounded two-principal filesystem check establishes the original and replacement effective ACL decisions on a supported OS. The original file must restrict the second user beyond basic permissions, the parent directories must permit traversal, the saving owner must be able to replace the inode, inherited/default ACLs must not preserve the restriction, and the in-place fallbacks must not apply. Source alone does not establish those environmental conditions.

#### Bounded validation plan

**Local:** After all required containment controls are established, use an ACL-capable scratch filesystem and two dummy principals with only non-secret marker data. Record the file and parent ACLs, mode, uid/gid and link count, and establish that the saving owner may write/replace the file while the second principal cannot read it. Exercise only the ordinary replacement path, using the existing write_atomic_elsewhere test seam or an equivalent scratch path outside configured temporary directories; confirm that rename succeeded rather than an in-place fallback. Compare the same second-principal access decision before and after, including the inherited/default ACL on the new inode. Stop at this one dummy-file observation. Include a directory-inherited restrictive-ACL control and an in-place-path control. Use no real user files or credentials, no network, read-only source/toolchain, empty allowlisted environment, scratch-only writes, and strict resource and process-tree limits. Do not infer access widening merely from missing ACL entries when another layer still denies access.
