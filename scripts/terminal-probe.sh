#!/bin/sh
# Asks the terminal this runs in for what TERMINALS.md's checks depend on,
# and prints a report keyed by check number to paste into the matrix.
#
# Most of it the terminal answers itself: replies to queries (XTVERSION, the
# Kitty keyboard flags after the push binvim makes, DECRQM modes, DECRQSS for
# SGR and cursor shape) and CSI 6n cursor reports around the fixture's
# clusters. The rest needs a person: a few keys, a click, a paste, and y/n on
# what is drawn. Every prompt gives up after 20 seconds.
#
# Every query is followed by a DA1 request, which all terminals answer, so a
# query the terminal ignores costs one round trip rather than a timeout.
#
# POSIX sh, so it runs as it is on macOS, Linux, WSL and over SSH.

set -u

report_file=${TMPDIR:-/tmp}/binvim-terminal-probe.txt
esc=$(printf '\033')
cr=$(printf '\r')
csi="$esc["
st="$esc\\"
nl=$(printf '\n.')
nl=${nl%.}

if ! saved=$(stty -g </dev/tty 2>/dev/null); then
  echo "terminal-probe: needs a terminal" >&2
  exit 1
fi

out() { printf '%s' "$*" >/dev/tty; }
say() { printf '%s\r\n' "$*" >/dev/tty; }

pushed=0
restore() {
  if [ "$pushed" = 1 ]; then out "$csi<u"; fi
  out "$csi?1000l$csi?1006l$csi?2004l${csi}0m${csi}0 q"
  stty "$saved" </dev/tty
}
trap restore EXIT
trap 'exit 130' INT TERM HUP
stty raw -echo </dev/tty
stty opost min 0 time 1 </dev/tty

# One read of whatever has arrived, waiting up to a tenth of a second. The
# dot keeps a newline at the end of the read from being stripped.
chunk() {
  buf=$(dd bs=512 count=1 2>/dev/null </dev/tty; printf .)
  buf=${buf%.}
}

# Sends a query and collects the reply up to the DA1 answer, which is dropped.
ask() {
  out "$1${csi}c"
  reply='' idle=0
  while [ "$idle" -lt 20 ]; do
    chunk
    if [ -z "$buf" ]; then
      idle=$((idle + 1))
      continue
    fi
    idle=0
    reply=$reply$buf
    case $reply in *"$csi?"*) ;; *) continue ;; esac
    tail=${reply##*"$csi?"}
    case $tail in *[!0-9\;c]*) ;; *c) break ;; esac
  done
  reply=${reply%"$csi?"*}
}

# Reads until what has arrived matches the pattern in $1, for up to $2 tenths
# of a second. The pattern leaves out the reply's `ESC [`, whose `[` would
# open a bracket expression.
wait_for() {
  got='' n=0
  while [ "$n" -lt "$2" ]; do
    chunk
    got=$got$buf
    case $got in $1) return 0 ;; esac
    n=$((n + 1))
  done
  return 1
}

# Waits up to 20 seconds for a key, then takes the rest of its sequence.
readkey() {
  key='' n=0
  while [ -z "$key" ] && [ "$n" -lt 200 ]; do
    chunk
    key=$buf
    n=$((n + 1))
  done
  while [ -n "$key" ]; do
    chunk
    [ -z "$buf" ] && break
    key=$key$buf
  done
}

shown() { printf '%s' "$1" | cat -vt; }

yn() {
  out "$1 [y/n] "
  readkey
  case $key in
    y | Y) ans=yes ;;
    n | N) ans=no ;;
    *) ans='?' ;;
  esac
  say "$ans"
}

rows=''
row() { rows=$rows$(printf '%2s  %-20s %s\t%s' "$1" "$2" "$3" "$4")$nl; }

say ''
say 'binvim terminal probe. Keep this window focused; each prompt gives up after 20 seconds.'
say ''

ask "$csi>0q"
case $reply in
  *"${esc}P>|"*)
    name=${reply#*"${esc}P>|"}
    name=${name%%"$st"*}
    ;;
  *) name='not reported' ;;
esac

flags_of() {
  case $1 in
    *"$csi?"*u*)
      f=${1#*"$csi?"}
      f=${f%%u*}
      ;;
    *) f=none ;;
  esac
}
ask "$csi?u"
flags_of "$reply"
flags_before=$f
out "$csi>1u"
pushed=1
ask "$csi?u"
flags_of "$reply"
flags_after=$f

# 1 or 2 is a mode the terminal knows; 0 is one it doesn't, 4 one it never sets.
# `none` is no answer at all, which tmux gives for every mode.
mode() {
  ask "$csi?$1\$p"
  case $reply in
    *"$csi?$1;"*)
      m=${reply#*"$csi?$1;"}
      m=${m%%\$*}
      ;;
    *) m=none ;;
  esac
}
mode 2026
sync_mode=$m
mode 2004
paste_mode=$m
mode 1006
mouse_mode=$m

sgr() {
  out "${csi}0m$1"
  ask "${esc}P\$qm$st"
  out "${csi}0m"
  case $reply in
    *"${esc}P1\$r"*)
      s=${reply#*"${esc}P1\$r"}
      s=${s%%"$st"*}
      ;;
    *) s=none ;;
  esac
}
sgr "${csi}38;2;10;20;30m"
truecolor_sgr=$s
sgr "${csi}4:3m"
undercurl_sgr=$s

out "${csi}6 q"
ask "${esc}P\$q q$st"
out "${csi}0 q"
case $reply in *"1\$r6 q"*) shape_rq=yes ;; *) shape_rq=no ;; esac

# Cell widths as binvim gives them (`render::cluster_width`, over
# unicode-width): the CSI 6n column after drawing each one from column 1.
zwj=$(printf '\342\200\215')
family="👨${zwj}👩${zwj}👧"
heart="❤$(printf '\357\270\217')"
acute="e$(printf '\314\201')"
glyphs="$(printf '\356\202\260 \356\202\262 \356\232\227 \357\205\233')"
width() {
  out "$cr$csi""2K$1"
  ask "${csi}6n"
  out "$cr$csi""2K"
  case $reply in
    *R*)
      c=${reply##*;}
      c=${c%%R*}
      w=$((c - 1))
      ;;
    *) w='?' ;;
  esac
}
widths_ok=yes
widths=''
measure() {
  width "$1"
  widths="$widths $1 $w"
  [ "$w" = "$2" ] || widths_ok=no
}
measure '漢字かなカナ한글' 16
wide_ok=$widths_ok
wide=$widths
widths_ok=yes
widths=''
measure '👍🏽' 2
measure "$family" 2
measure '🇮🇸' 2
measure "$heart" 2
measure "$acute" 1
emoji_ok=$widths_ok
emoji=$widths
width "$(printf '\356\202\260')"
glyph_w=$w

say 'Look at this line:'
say ''
bar='' i=0
while [ "$i" -lt 48 ]; do
  bar="$bar${csi}48;2;$((i * 5));$((60 + i * 2));$((230 - i * 4))m "
  i=$((i + 1))
done
say "   ${csi}3mitalic${csi}0m   ${csi}1mbold${csi}0m   ${csi}4:3m${csi}58;2;243;139;168mwavy${csi}0m   icons: $glyphs"
say "   $bar${csi}0m"
say ''
yn "Is 'italic' slanted?"
italic=$ans
yn "Is 'bold' bold?"
bold=$ans
yn "Does 'wavy' have a curly (not straight) pink underline?"
curly=$ans
yn "Do the four icons draw as symbols, not boxes or question marks?"
icons=$ans
yn "Does the colour bar fade smoothly, without stripes of flat colour?"
smooth=$ans
out "${csi}6 q"
yn "Is the cursor a thin bar right now?"
bar_cursor=$ans
out "${csi}0 q"
yn "Is there stray text anywhere on screen that this probe didn't mean to print (like [>1u)?"
stray=$ans

say ''
say "Press each key when asked."
press() {
  out "  $1: "
  readkey
  if [ -n "$key" ]; then say "$(shown "$key")"; else say '(nothing)'; fi
}
press 'Esc'
k_esc=$key
press 'Ctrl-['
k_ctrl_bracket=$key
press 'Tab'
k_tab=$key
press 'Ctrl-i'
k_ctrl_i=$key
press 'Ctrl-w'
k_ctrl_w=$key
press 'Ctrl-c'
k_ctrl_c=$key
press 'Alt-Backspace (Option on a Mac)'
k_alt_bs=$key
press 'Shift-Right'
k_shift_right=$key
press 'Ctrl-Right'
k_ctrl_right=$key
press 'Shift-Left'
k_shift_left=$key
press 'Ctrl-Left'
k_ctrl_left=$key

say ''
out "$csi?1000h$csi?1006h"
out 'Click the X: '
ask "${csi}6n"
p=${reply#*"$csi"}
x_row=${p%%;*}
x_col=${p#*;}
x_col=${x_col%%R*}
say 'X'
click='nothing arrived'
if wait_for "*<0;*[Mm]*" 200; then
  p=${got#*"$csi<0;"}
  mx=${p%%;*}
  p=${p#*;}
  my=${p%%[Mm]*}
  click="at $mx,$my (X is at $x_col,$x_row)"
fi
say 'Scroll the mouse wheel over this window.'
wheel=no
if wait_for "*<6[45];*" 200; then wheel=yes; fi
out "$csi?1000l$csi?1006l"

# A paste of text the probe wrote through OSC 52 answers both checks at once:
# the paste shows the terminal brackets it, and its content that the write
# reached the clipboard.
token="probe$$"
b64=$(printf '%s one\n%s two' "$token" "$token" | base64 | tr -d '\n')
seq="${esc}]52;c;$b64$(printf '\007')"
out "$seq"
if [ -n "${TMUX:-}" ]; then
  out "${esc}Ptmux;$(printf '%s' "$seq" | sed "s/$esc/$esc$esc/g")$st"
fi
out "$csi?2004h"
say ''
say "Paste now with the terminal's paste key (Cmd-V, Ctrl-Shift-V, or right-click)."
bracketed=no
if wait_for '*201~*' 200; then bracketed=yes; fi
pasted=$got
out "$csi?2004l"
case $pasted in
  *"$token one"*"$token two"*) osc52=yes ;;
  *) osc52=no ;;
esac
breaks=''
case $pasted in *"$cr"*) breaks="$breaks CR" ;; esac
case $pasted in *"$nl"*) breaks="$breaks LF" ;; esac

restore
trap - EXIT

verdict() {
  case $1 in
    yes) v='✓' ;;
    no) v='✗' ;;
    *) v='?' ;;
  esac
}

in_set() {
  k=$1
  shift
  for e in "$@"; do [ "$k" = "$e" ] && return 0; done
  return 1
}

del=$(printf '\177')
if in_set "$k_ctrl_w" "$(printf '\027')" "${csi}119;5u" &&
  in_set "$k_tab" "$(printf '\t')" &&
  in_set "$k_ctrl_i" "$(printf '\t')" "${csi}105;5u" &&
  in_set "$k_ctrl_c" "$(printf '\003')" "${csi}99;5u"; then v='✓'; else v='✗'; fi
row 2 'Re-encoded keys' "$v" "Ctrl-w $(shown "$k_ctrl_w")  Tab $(shown "$k_tab")  Ctrl-i $(shown "$k_ctrl_i")  Ctrl-c $(shown "$k_ctrl_c")"

if in_set "$k_esc" "$esc" "${csi}27u"; then v='✓'; else v='✗'; fi
row 3 'Esc response' "$v" "Esc arrives as $(shown "$k_esc")"

if ! in_set "$k_alt_bs" "$esc$del" "$esc$(printf '\010')" "${csi}127;3u"; then
  v='✗'
  alt_note="Alt-Backspace $(shown "$k_alt_bs"): the Option key isn't sending Alt"
elif in_set "$k_ctrl_bracket" "$esc" "${csi}91;5u"; then
  v='✓'
  alt_note="Ctrl-[ $(shown "$k_ctrl_bracket")  Alt-Backspace $(shown "$k_alt_bs")"
else
  v='✗'
  alt_note="Ctrl-[ $(shown "$k_ctrl_bracket")"
fi
row 4 'Ctrl-[ and Alt' "$v" "$alt_note"

arrows_ok=yes arrows_taken=''
arrow() {
  if [ -z "$2" ]; then
    arrows_taken="$arrows_taken $1"
  elif [ "$2" != "$3" ]; then
    arrows_ok=no
  fi
}
arrow Shift-Right "$k_shift_right" "${csi}1;2C"
arrow Ctrl-Right "$k_ctrl_right" "${csi}1;5C"
arrow Shift-Left "$k_shift_left" "${csi}1;2D"
arrow Ctrl-Left "$k_ctrl_left" "${csi}1;5D"
if [ "$arrows_ok" = no ]; then
  v='✗'
elif [ -n "$arrows_taken" ]; then
  v='n/a'
else
  v='✓'
fi
row 5 'Shift / Ctrl arrows' "$v" "$(shown "$k_shift_right $k_ctrl_right $k_shift_left $k_ctrl_left")${arrows_taken:+  nothing arrived for$arrows_taken (an OS shortcut?)}"

verdict "$bar_cursor"
row 6 'Cursor shape' "$v" "you saw a bar: $bar_cursor, DECRQSS read back 6 q: $shape_rq"

case $truecolor_sgr in *10[:\;]20[:\;]30*) tc_rq=yes ;; *) tc_rq=no ;; esac
if [ "$tc_rq" = yes ] || [ "$smooth" = yes ]; then v='✓'; else v='✗'; fi
row 7 'Truecolor' "$v" "SGR read back: $tc_rq, smooth bar: $smooth, COLORTERM=${COLORTERM:-unset}"

if [ "$italic" = yes ] && [ "$bold" = yes ]; then v='✓'; else v='✗'; fi
row 8 'Italic and bold' "$v" "italic: $italic, bold: $bold"

verdict "$icons"
row 9 'Nerd Font glyphs' "$v" "you saw them draw: $icons, U+E0B0 is $glyph_w cell(s)"

verdict "$wide_ok"
row 10 'Wide characters' "$v" "$wide (binvim: 16)"

verdict "$emoji_ok"
row 11 'Emoji clusters' "$v" "$emoji (binvim: 2 2 2 2 1)"

if [ "$click" = "at $x_col,$x_row (X is at $x_col,$x_row)" ] && [ "$wheel" = yes ]; then v='✓'; else v='✗'; fi
row 13 'Mouse' "$v" "click $click, wheel: $wheel, DECRQM 1006 = $mouse_mode"

verdict "$bracketed"
row 14 'Bracketed paste' "$v" "breaks:${breaks:- none}, DECRQM 2004 = $paste_mode"

case $sync_mode in
  1 | 2) v='✓' ;;
  none) v='?' ;;
  *) v='✗' ;;
esac
row 15 'Synchronized output' "$v" "DECRQM 2026 = $sync_mode"

verdict "$curly"
row 16 'Undercurl' "$v" "you saw it curl: $curly, SGR read back: $undercurl_sgr"

verdict "$osc52"
row 18 'OSC 52' "$v" "the paste was the probe's OSC 52 text: $osc52"

if [ "$k_ctrl_bracket" = "${csi}91;5u" ]; then v='✓'; else v='KI'; fi
row 19 ':terminal Ctrl-[' "$v" "Ctrl-[ $(shown "$k_ctrl_bracket")"

report="terminal: $name
TERM=${TERM:-unset} TERM_PROGRAM=${TERM_PROGRAM:-unset} ${TERM_PROGRAM_VERSION:+TERM_PROGRAM_VERSION=$TERM_PROGRAM_VERSION }COLORTERM=${COLORTERM:-unset}${TMUX:+ (in tmux)}${SSH_CONNECTION:+ (over SSH)}
Kitty keyboard flags: $flags_before before binvim's push, $flags_after after. Stray text on screen: $stray.

$rows
Still to run inside binvim: checks 1, 12, 17, 20 and 21 in TERMINALS.md."

printf '\n%s\n' "$report" | tee "$report_file"
printf '\nSaved to %s\n' "$report_file"
