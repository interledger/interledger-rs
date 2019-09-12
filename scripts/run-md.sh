#!/bin/bash
# Based on mdlp.awk from https://gist.github.com/trauber/4955706
# Originally written by @trauber Rich Traube

# This script parses and executes code blocks found in Markdown files.
# In addition to parsing code found between ```bash ... ```,
# it also parses code from special HTML comments that start with <!--!
# This allows us to hide certain commands from view in the rendered
# Markdown file but run them when the whole file is executed.
# This is largely intended for commands that make the output more readable

TMP_SCRIPT=$(mktemp)
export RUN_MD_LIB="$(dirname $0)/run-md-lib.sh"

if [ -n "$1" ]; then
  # if the first argument is a file, run it
  if [ -f "$1"  ]; then
    MD_FILE="$1"
  elif [ -d "$1" ]; then
    # if the argument is a directory, run the README.md file in it
    MD_FILE="$1/README.md"
  fi
else
  MD_FILE=-
fi

cat "$MD_FILE" | awk 'BEGIN { one_line_comment="^<!--!(.*)-->$" }
{
  # cc = code block count
  # hc = html comment count
  # ps = print script
  # codeblock starts only if the block is of bash, and ends if the line starts with ```
  if (hc % 2 == 0 && /^```/) { if (/^```(bash)$/) { ps = 1; } else { ps = 0; } cc++; next }

  # html comment starts if the line starts with <!--! and the line is not one line comment (<!--! foo -->)
  else if (cc % 2 == 0 && /^<!--!/ && $0 !~ one_line_comment) { ps = 1; hc++; next }

  # html comment ends if the line starts with --> and it is in html comment context
  else if (cc % 2 == 0 && hc % 2 == 1 && /^-->/) { hc++; next }

  # if the line is in either html comment section or code block section, print the line
  else if ((hc % 2 == 1 || cc % 2 == 1) && ps == 1) { print }

  # if the line matches one line comment (<!--! foo -->), just print it
  else if ($0 ~ one_line_comment) { p = $0; sub("^<!--! *", "", p); sub(" *-->$", "", p); print p }
}' > "$TMP_SCRIPT"
bash "$TMP_SCRIPT"

if [ $? -eq 0 ]; then
  rm "$TMP_SCRIPT"
  exit 0
else
  printf "\e[31;1mError running markdown file: $MD_FILE (parsed bash script $TMP_SCRIPT)\e[m\n" 1>&2
  exit 1
fi
