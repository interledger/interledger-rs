#!/usr/bin/env bash

function create_tag() {
    local repository="interledger-rs/interledger-rs"
    local user_agent="curl-on-CircleCI"
    local tag_name=$1
    local message=$2
    local object=$3
    local type=$4
    local log_dir=${LOG_DIR:-logs}

    if [ -z "${tag_name}" ]; then
        printf "%s\n" "tag name is required."
        exit 1
    fi
    if [ -z "${message}" ]; then
        printf "%s\n" "message name is required."
        exit 1
    fi
    if [ -z "${object}" ]; then
        printf "%s\n" "object is required."
        exit 1
    fi
    if [ -z "${type}" ]; then
        printf "%s\n" "type is required."
        exit 1
    fi

    mkdir -p "${log_dir}"

    # check if there is any release of the same tag
    curl \
        -X GET \
        -H "User-Agent: ${user_agent}" \
        -H "Authorization: token ${GITHUB_OAUTH_TOKEN}" \
        -H "Accept: application/vnd.github.v3+json" \
        https://api.github.com/repos/${repository}/releases/tags/${tag_name} 2>/dev/null >${log_dir}/prev_tag.json || exit 2
    local release_id=$(jq -r .id < "${log_dir}/prev_tag.json")

    # delete it if found
    if [ "${release_id}" != "null" ]; then
        printf "%s%d%s\n" "Found a tag of the same name: " "${release_id}" ", deleting..."
        curl \
            -X DELETE \
            -H "User-Agent: ${user_agent}" \
            -H "Authorization: token ${GITHUB_OAUTH_TOKEN}" \
            -H "Accept: application/vnd.github.v3+json" \
            https://api.github.com/repos/${repository}/releases/${tag_name} 2>/dev/null >${log_dir}/delete_tag.json || exit 2
    fi

    # create a new tag
    json=$(printf '{
      "tag": "%s",
      "message": "%s",
      "object": "%s",
      "type": "%s"
    }' "${tag_name}" "${message}" "${object}" "${type}")

    printf "%s\n" "Creating a tag: ${tag_name}..."
    curl \
        -X POST \
        -H "User-Agent: ${user_agent}" \
        -H "Authorization: token ${GITHUB_OAUTH_TOKEN}" \
        -H "Accept: application/vnd.github.v3+json" \
        -d "${json}" \
        https://api.github.com/repos/${repository}/git/tags 2>/dev/null >${log_dir}/tag.json || exit 2
}

create_tag "$@"
