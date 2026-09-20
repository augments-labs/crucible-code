#!/usr/bin/env bash
# Ask the registry for every container image the release workflow pins by
# digest, and fail when one no longer resolves. A registry garbage-collects the
# manifest behind a moving tag on its own schedule, so a pin can die with no
# change here; this finds that on the weekly canary rather than on a tag.
#
#   scripts/sh/release-container.sh [workflow]
#
# A relative [workflow] is read from the repository root, wherever this is run
# from. Exits 0 when every pin resolves to the manifest it names and non-zero
# otherwise, including when a pin is spelt in a way this cannot read or the
# workflow cannot be read at all.
set -euo pipefail

cd "$(dirname "$0")/../.."
workflow=${1:-.github/workflows/release.yml}
accept='application/vnd.oci.image.index.v1+json, application/vnd.docker.distribution.manifest.list.v2+json, application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json'
pinned='[[:space:]]*image:[[:space:]]*\([^[:space:]#]*@sha256:[0-9a-f]\{64\}\)[[:space:]]*$'

# Every line that names an image, in the mapping form or the short one, has to
# be one the pattern reads. One that is quoted, carries a comment, comes from an
# expression or has no digest would otherwise go unchecked beside one that does.
unread=$(grep -n -E '^[[:space:]]*(image|container):[[:space:]]*[^[:space:]#]' "$workflow" | grep -v "^[0-9]*:$pinned" || true)
[[ -z $unread ]] || {
    printf '%s names a container image this check cannot read; write it as `image: <name>@sha256:<digest>` with nothing after it:\n%s\n' \
        "$workflow" "$unread" >&2
    exit 1
}

images=$(sed -n "s/^$pinned/\\1/p" "$workflow" | sort -u)
[[ -n $images ]] || {
    printf '%s pins no container image by digest\n' "$workflow" >&2
    exit 1
}

if command -v sha256sum >/dev/null; then
    sum=(sha256sum)
else
    sum=(shasum -a 256)
fi
manifest=$(mktemp)
trap 'rm -f "$manifest"' EXIT

failed=0
while read -r image; do
    digest=${image##*@}
    name=${image%@*}
    registry=${name%%/*}
    repository=${name#*/}
    repository=${repository%%:*}
    status=$(curl --silent --show-error --output "$manifest" --write-out '%{http_code}' \
        --max-time 30 --retry 3 --header "Accept: $accept" \
        "https://$registry/v2/$repository/manifests/$digest") || status=000
    case $status in
        200)
            # A digest names the bytes of its manifest, so an answer that is
            # not those bytes is not the image, whatever the status says.
            answered="sha256:$("${sum[@]}" <"$manifest" | cut -d' ' -f1)"
            if [[ $answered == "$digest" ]]; then
                printf '%s resolves\n' "$image"
            else
                printf '%s could not be checked: the registry answered 200 with a body that digests to %s\n' \
                    "$image" "$answered" >&2
                failed=1
            fi
            ;;
        404)
            printf '%s no longer resolves: the registry answered 404. Resolve the tag again and repoint the pin in %s.\n' \
                "$image" "$workflow" >&2
            failed=1
            ;;
        *)
            printf '%s could not be checked: the registry answered %s\n' "$image" "$status" >&2
            failed=1
            ;;
    esac
done <<<"$images"
exit "$failed"
