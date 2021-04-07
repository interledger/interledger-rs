#!/bin/bash

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

# run tcpdump in the same directory where other artifacts to be uploaded reside
PARENT_DIR_NAME="${PWD##*/}"
TCPDUMP_OUTPUT_FILENAME="$PARENT_DIR_NAME".pcap
echo "saving packet capture for $PARENT_DIR_NAME/${MD_FILE##*/} as $TCPDUMP_OUTPUT_FILENAME"
sudo tcpdump -i lo -s 65535 -w "/tmp/run-md-test/${TCPDUMP_OUTPUT_FILENAME}" &
TCPDUMP_PID=$!

cat "$MD_FILE" | "$(dirname $0)/parse-md.sh" > "$TMP_SCRIPT"
bash -x -O expand_aliases "$TMP_SCRIPT"

sudo kill -2 $TCPDUMP_PID
if [ $? -eq 0 ]; then
  rm "$TMP_SCRIPT"
  exit 0
else
  printf "\e[31;1mError running markdown file: $MD_FILE (parsed bash script $TMP_SCRIPT)\e[m\n" 1>&2
  exit 1
fi
