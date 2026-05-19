#!/bin/bash
cargo check -p llm-router 2>&1
echo "---EXIT_CODE: $?---"
