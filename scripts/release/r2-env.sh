# Sourced by the publish scripts: an rclone remote named "r2" for Cloudflare
# R2, from R2_ACCOUNT_ID, R2_ACCESS_KEY_ID and R2_SECRET_ACCESS_KEY.
: "${R2_ACCOUNT_ID:?}" "${R2_ACCESS_KEY_ID:?}" "${R2_SECRET_ACCESS_KEY:?}"
export RCLONE_CONFIG_R2_TYPE=s3
export RCLONE_CONFIG_R2_PROVIDER=Cloudflare
export RCLONE_CONFIG_R2_ACCESS_KEY_ID="$R2_ACCESS_KEY_ID"
export RCLONE_CONFIG_R2_SECRET_ACCESS_KEY="$R2_SECRET_ACCESS_KEY"
export RCLONE_CONFIG_R2_ENDPOINT="https://$R2_ACCOUNT_ID.r2.cloudflarestorage.com"
export RCLONE_CONFIG_R2_NO_CHECK_BUCKET=true
export RCLONE_CONFIG_R2_ACL=private

# Uploads a local folder to r2:<dest> so clients never see an index that
# names files not uploaded yet: content first, then the files matching the
# given index patterns, then removal of what the local folder dropped.
r2_publish() {
    src=$1 dest=$2
    shift 2
    excludes=""
    for pattern in "$@"; do
        excludes="$excludes --exclude $pattern"
    done
    # shellcheck disable=SC2086
    rclone copy --checksum --transfers 16 $excludes "$src" "r2:$dest"
    rclone copy --checksum "$src" "r2:$dest"
    rclone sync --checksum "$src" "r2:$dest"
}
