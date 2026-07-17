const CBC_TAG = "latest-cbc";
const REPO = "clyso/cbs";
const BASE_API_URL = `https://api.github.com/repos/${REPO}`;
const TAGS_URL = `${BASE_API_URL}/tags`;
const RELEASES_URL = `${BASE_API_URL}/releases/tags`;



async function getLatestCBCAssets() {
  var release_cbc_tag;

  const gh_headers = {
    headers: {
      "Accept": "application/vnd.github.v3+json",
      "X-GitHub-Api-Version": "2026-03-10"
    }
  };

  try {
    const response = await fetch(TAGS_URL, gh_headers);
    if (!response.ok) {
      throw new Error(`GitHub API error: ${response.status}`);
    }

    const data = await response.json();
    const tag_entry = data.find(entry => entry.name == CBC_TAG);
    if (!tag_entry) {
      // make this an error banner instead of download buttons
      throw new Error(`Tag ${CBC_TAG} not found in GitHub API response`);
    }
    const latest_sha = tag_entry.commit.sha;

    const latest_cbc_tag_entry = data.find(entry =>
      entry.name.startsWith("cbc-v") &&
      entry.commit.sha == latest_sha
    );
    if (!latest_cbc_tag_entry) {
      // make this an error banner instead of download buttons
      throw new Error(`No cbc tag found for commit ${latest_sha}`);
    }
    release_cbc_tag = latest_cbc_tag_entry.name;

  } catch (error) {
    console.error("Error fetching latest release:", error);
    throw new Error("Error fetching latest release");
  }

  if (!release_cbc_tag) {
    throw new Error("No release tag found for CBC");
  }

  var arch_assets = {};
  try {
    const response = await fetch(RELEASES_URL + `/${release_cbc_tag}`, gh_headers);
    if (!response.ok) {
      // make this an error banner instead of download buttons
      throw new Error(`GitHub API error: ${response.status}`);
    }

    const data = await response.json();
    const assets = data.assets.filter(entry =>
      entry.name == "cbc-linux-amd64" ||
      entry.name == "cbc-macos-arm64"
    );

    assets.map(asset => {
      const arch = asset.name.match(/^cbc-(.*)$/);
      if (!arch) {
        throw new Error(`Unexpected asset name format: ${asset.name}`);
      }
      arch_assets[arch[1]] = asset.browser_download_url;
    });

  } catch (error) {
    console.error("Error fetching release details:", error);
    throw new Error(`Error fetching release '${release_cbc_tag}' details`);
  }

  if (!arch_assets) {
    throw new Error(
      `No architecture assets found for CBC release ${release_cbc_tag}`
    );
  }

  return {
    version: release_cbc_tag,
    assets: arch_assets
  };
}

async function setDownloadError(message) {
  var warn_box = document.getElementById("cbc-download-warn-box");
  var warn_msg = document.getElementById("cbc-download-warn-msg");
  var download_btns = document.getElementById("cbc-download-btns");

  warn_msg.textContent = message;
  warn_box.style.display = "block";
  download_btns.style.display = "none";
}

async function prepareDownloadLocations() {
  let assets;
  try {
    assets = await getLatestCBCAssets();
  } catch (error) {
    await setDownloadError(error.message);
  }

  console.debug("cbc assets:", assets);

  let linux_amd64_btn = document.getElementById("cbc-download-url-linux-amd64");
  let macos_arm64_btn = document.getElementById("cbc-download-url-macos-arm64");
  let cbc_version_span = document.getElementById("cbc-version");

  cbc_version_span.textContent = assets.version;

  let has_linux_amd64 = ("linux-amd64" in assets.assets);
  let has_macos_arm64 = ("macos-arm64" in assets.assets);

  if (has_linux_amd64) {
    linux_amd64_btn.href = assets.assets["linux-amd64"];
  } else {
    linux_amd64_btn.textContent = "Linux AMD64 not available";
  }

  if (has_macos_arm64) {
    macos_arm64_btn.href = assets.assets["macos-arm64"];
  } else {
    macos_arm64_btn.textContent = "macOS ARM64 not available";
  }
}
