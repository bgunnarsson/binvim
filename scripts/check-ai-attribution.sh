#!/usr/bin/env bash
set -euo pipefail

# Fails when anything in the repo credits an AI with the work: a commit
# message or author/committer identity anywhere in HEAD's history, a
# tracked file, or — when the workflow sets them — the PR title, PR body
# and branch name (PR_TITLE / PR_BODY / BRANCH).
#
# `--message <file>` is the commit-msg hook's mode (.githooks/commit-msg):
# only the message being written, the identity writing it and the current
# branch. The rest of the repo is CI's job, and rescanning the whole
# history on every commit would make the hook slow enough to get skipped.
#
# binvim itself integrates AI tools (the :claude / :codex / :opencode
# panes, Copilot ghost text), so naming one is not an offence. The rules
# only fire on the shapes attribution takes: `*-by:` trailers, bot and
# vendor identities, and "generated with X"-style phrases. Tool names
# that are also plain words or first names (Cursor, Amp, Devin, …) only
# count capitalised and straight after such a phrase — "moved by cursor
# keys" passes, "Generated with Cursor" does not.
#
# This file is the one place the patterns may appear, so the tree scan
# skips it. Patterns go through `-e` because the first one starts with a
# dash and grep would otherwise read it as an option.

cd "$(git rev-parse --show-toplevel)"
self='scripts/check-ai-attribution.sh'

tools='claude|anthropic|chatgpt|gpt(-?[0-9o][a-z0-9.-]*)?|openai|codex|copilot|gemini|aider|devin-ai|google-labs-jules|windsurf|codeium|tabnine|codewhisperer|amazon q|cline|roo ?code|opencode|ampcode|deepseek|qwen|mistral|llama|perplexity|supermaven|openhands|coderabbit|cursor ?agent|cursor ai'
words='Cursor|Amp|Goose|Crush|Droid|Sweep|Continue|Bolt|Kiro|Cody|Kimi|Grok|Devin|Jules|Junie|Augment|Lovable'
generic='ai|a\.i\.|llm|large language model|language model|artificial intelligence|chatbot|ai (assistant|agent)|coding (assistant|agent)'
domains='anthropic\.com|openai\.com|cursor\.(com|sh)|aider\.chat|opencode\.ai|ampcode\.com|charm\.land|codeium\.com|windsurf\.com|tabnine\.com|sourcegraph\.com|cognition\.ai|devin\.ai|factory\.ai|x\.ai|mistral\.ai|deepseek\.com'
identity="\b($tools)\b|@([a-z0-9-]+\.)*($domains)\b"

lead='(generated|written|authored|co-?authored|created|produced|drafted|coded|made|built|assisted|powered|developed|implemented|refactored|reviewed)[[:space:]]+(with|by|using|via|through|alongside)|(thanks|thank you|credit|kudos|courtesy)[[:space:]]+(to|of|goes to)|with[[:space:]]+(the[[:space:]]+)?(help|assistance|support)[[:space:]]+(of|from)|help[[:space:]]+from'
object='[[:space:]]+(the[[:space:]]+|an?[[:space:]]+)?\[?'

rules=(
    # Co-authored-by: / Signed-off-by: / … naming a tool, a vendor or a bot.
    "-by[[:space:]]*:.*(\b($tools|$generic|bot|agent|assistant)\b|\[bot\])"
    # Trailers that only exist to disclose AI involvement, whatever their
    # value. Not `\b`: the hyphen in a human's Co-authored-by: is a word
    # boundary, and `authored-by:` would match it.
    "(^|[^-[:alnum:]])(assisted|generated|written|created|authored)-(by|with|using)[[:space:]]*:"
    "\bai-(assisted|generated|model|tool|agent|disclosure)[[:space:]]*:"
    "\b(amp|$tools)-(thread|session|model|prompt|conversation|chat)(-id)?[[:space:]]*:"
    # An AI vendor's address, or a tool's GitHub bot account.
    "@([a-z0-9-]+\.)*($domains)\b"
    "\b($tools)[a-z0-9-]*(\[bot\])?@users\.noreply\.github\.com"
    # "Generated with Claude Code", "written by an AI", "thanks to ChatGPT".
    "\b($lead)$object($tools|$generic)\b"
    "\b(ai|llm|gpt)[- ](generated|written|authored|assisted|created|produced|coded)\b"
)
insensitive=$(IFS='|'; printf '%s' "${rules[*]}")

matches() {
    local text
    text=$(cat)
    {
        grep -iE -e "$insensitive" <<<"$text" || true
        grep -iE -e "\b($lead)$object($words)\b" <<<"$text" | grep -E -e "\b($words)\b" || true
    } | awk '!seen[$0]++'
}

failed=0
report() {
    [[ -n $2 ]] || return 0
    printf '%s\n%s\n\n' "$1" "$2" >&2
    failed=1
}

if [[ ${1:-} == --message ]]; then
    # git hands the hook the raw editor file, so the comment lines and a
    # `commit -v` diff below the scissors line are still in it — and that
    # diff is the change, not the message.
    report 'Commit message:' "$(
        sed '/^. -\{24\} >8 -\{24\}$/,$d' "$2" | git stripspace --strip-comments | matches
    )"

    report 'Commit author / committer:' "$(
        { git var GIT_AUTHOR_IDENT; git var GIT_COMMITTER_IDENT; } |
            grep -iE -e "$identity" || true
    )"

    BRANCH=$(git symbolic-ref -q --short HEAD || true)
else
    report 'Commit messages:' "$(
        git log --format='%x01%h%n%B' HEAD |
            awk '/^\001/ { sha = substr($0, 2); next } { print sha ": " $0 }' |
            matches
    )"

    report 'Commit authors / committers:' "$(
        git log --format='%h: %an <%ae>%n%h: %cn <%ce>' HEAD |
            grep -iE -e "$identity" || true
    )"

    report 'Tracked files:' "$(git grep -I -n -e '' -- . ":(exclude)$self" | matches)"

    report 'PR title:' "$(printf '%s\n' "${PR_TITLE:-}" | matches)"
    report 'PR description:' "$(printf '%s\n' "${PR_BODY:-}" | matches)"
fi

# Agents push to a branch namespaced by the tool — claude/…, codex/…,
# cursor/… — so a tool name as the leading segment is attribution even
# though "fix-claude-pane" further in is not.
report 'Branch name:' "$(
    printf '%s\n' "${BRANCH:-}" |
        grep -iE -e "^($tools|$words)/" || true
)"

if ((failed)); then
    echo 'AI attribution found. Reword what is listed above and drop the trailers.' >&2
    exit 1
fi
echo 'No AI attribution found.'
