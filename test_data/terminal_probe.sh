#!/usr/bin/env sh
set -u

printf '== tty ==\n'
printf 'tty=%s\n' "$(tty 2>/dev/null || printf 'unknown')"
if [ -t 0 ]; then printf 'stdin: tty\n'; else printf 'stdin: not a tty\n'; fi
if [ -t 1 ]; then printf 'stdout: tty\n'; else printf 'stdout: not a tty\n'; fi
if [ -t 2 ]; then printf 'stderr: tty\n'; else printf 'stderr: not a tty\n'; fi

printf '\n== environment ==\n'
for name in TERM COLORTERM TERM_PROGRAM LANG LC_ALL LC_CTYPE NO_COLOR FORCE_COLOR CLICOLOR_FORCE; do
    eval "value=\${$name-}"
    printf '%s=%s\n' "$name" "$value"
done

printf '\n== terminal size/capabilities ==\n'
printf 'stty size: '
stty size 2>/dev/null || printf 'failed'
printf '\n'
printf 'tput colors: '
tput colors 2>/dev/null || printf 'failed'
printf '\n'

printf '\n== styles ==\n'
printf '\033[1mbold\033[0m '
printf '\033[3mitalic\033[0m '
printf '\033[4munderline\033[0m '
printf '\033[7mreverse\033[0m\n'

printf '\n== unicode ==\n'
printf 'emoji: 😀 😁 🚀\n'
printf 'cjk: 你好 こんにちは 안녕하세요\n'
printf 'combining: é å ö\n'
printf 'box: ┌────┬────┐\n'
printf '     │ ok │ 42 │\n'
printf '     └────┴────┘\n'

printf '\n== truecolor gradient ==\n'
i=0
while [ "$i" -le 79 ]; do
    r=$((255 - i * 3))
    g=$((i * 3))
    printf '\033[48;2;%d;%d;128m \033[0m' "$r" "$g"
    i=$((i + 1))
done
printf '\n'

printf '\n== 256-color cube ==\n'
i=16
while [ "$i" -le 231 ]; do
    printf '\033[48;5;%sm%3d\033[0m' "$i" "$i"
    if [ $(((i - 15) % 6)) -eq 0 ]; then
        printf '\n'
    fi
    i=$((i + 1))
done

printf '\n== prompt input ouk ==\n'
printf 'Type something and press enter: '
IFS= read -r reply
printf 'read back: %s\n' "$reply"
