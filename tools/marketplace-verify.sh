#!/bin/bash
# Asks the Omarchy plugin marketplace to verify and publish the current
# commit (run by .github/workflows/marketplace.yml on a version tag).
#
# The request is the marketplace's "Plugin verification" issue form, written
# byte for byte as the form renders it: the marketplace bot rejects requests
# whose headings differ. An open request from the same account is edited to
# the new commit instead of opening another one; editing re-runs validation.
#
# The marketplace only accepts the repository's current default-branch HEAD,
# so push main before the tag. Fails when the commit is not that HEAD, or
# when the listing records another repository path (after a rename): the
# marketplace rejects those until a maintainer records the migration. Does
# nothing when the marketplace already lists this commit.
#
# Env: GH_TOKEN (a token that can open issues on the marketplace repository),
# GITHUB_REPOSITORY (owner/name, set by Actions), TAG (optional: must match
# manifest.json), DRY_RUN=1 to print the request instead of sending it.
set -euo pipefail

marketplace=omacom/omarchy-plugin-marketplace
root=$(cd "$(dirname "$0")/.." && pwd)
plugin_id=$(jq -r .id "$root/manifest.json")
version=$(jq -r .version "$root/manifest.json")
name=$(jq -r .name "$root/manifest.json")
commit=$(git -C "$root" rev-parse HEAD)
repo=${GITHUB_REPOSITORY:?set GITHUB_REPOSITORY to owner/name}

if [[ -n ${TAG:-} && $TAG != "v$version" ]]; then
  echo "::error::tag $TAG does not match manifest.json version $version"
  exit 1
fi

head=$(git ls-remote "https://github.com/$repo" HEAD | cut -f1)
if [[ $commit != "$head" ]]; then
  echo "::error::$commit is not the current HEAD of $repo ($head); the marketplace only accepts that. Push main first, then tag its HEAD."
  exit 1
fi

# The API, not raw.githubusercontent.com: that one is cached for minutes.
registry=$(curl -fsSL -H 'Accept: application/vnd.github.raw' "https://api.github.com/repos/$marketplace/contents/registry.json")
listing=$(jq -c --arg id "$plugin_id" \
  '[.sources[] | select(.plugins? and (.plugins | has($id)))][0] // empty' <<<"$registry")
if [[ -z $listing ]]; then
  echo "::error::$plugin_id is not listed in the marketplace registry"
  exit 1
fi
listed_repo=$(jq -r '.repo | sub("^https://github.com/"; "")' <<<"$listing")
listed_commit=$(jq -r '.listingValidatedCommit // ""' <<<"$listing")

if [[ ${listed_repo,,} != "${repo,,}" ]]; then
  echo "::error::the marketplace lists $plugin_id at $listed_repo, not $repo; a maintainer must record the repository migration before an update can pass"
  exit 1
fi
if [[ $listed_commit == "$commit" ]]; then
  echo "the marketplace already lists $commit; nothing to do"
  exit 0
fi

title="[Verify]: $name v$version"
body=$(cat <<EOF
### Verification action

Verify and publish a newer upstream commit

### Plugin ID

$plugin_id

### Repository URL

https://github.com/$listed_repo

### Target commit

$commit

### Verification acknowledgment

- [x] I understand that only the exact target commit can become a verified marketplace snapshot and that verification is not a security audit.

### Standard installation acknowledgment

- [ ] I confirm that this listed root plugin supports the standard Omarchy installation path and does not require manual setup.
EOF
)

if [[ ${DRY_RUN:-} == 1 ]]; then
  printf 'would send:\n%s\n\n%s\n' "$title" "$body"
  exit 0
fi

: "${GH_TOKEN:?add MARKETPLACE_TOKEN to the marketplace environment (see AGENTS.md)}"
open=$(gh issue list -R "$marketplace" --author @me --state open --limit 100 --json number,body |
  jq -r --arg id "$plugin_id" '[.[] | select((.title | startswith("[Verify]:")) and (.body | contains("### Plugin ID\n\n" + $id + "\n")))][0].number // empty')
if [[ -n $open ]]; then
  gh issue edit "$open" -R "$marketplace" --title "$title" --body "$body" >/dev/null
  echo "updated https://github.com/$marketplace/issues/$open to $commit"
else
  url=$(gh issue create -R "$marketplace" --title "$title" --body "$body")
  echo "opened $url for $commit"
fi
