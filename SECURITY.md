# Security policy

## Supported versions

Only the latest release on [GitHub Releases](https://github.com/IconRunner05/cleanshot/releases)
gets security fixes. Before v1.0 every release is a pre-release.

## Reporting a vulnerability

Report it privately through GitHub Security Advisories: open the repository's **Security** tab
and choose **Report a vulnerability**
([direct link](https://github.com/IconRunner05/cleanshot/security/advisories/new)).
Do not open a public issue, pull request or discussion about it.

Please include the Cleanshot version, the Windows version and build, and the steps that show
the problem. A screenshot or a proof-of-concept file helps.

## What happens next

- We reply within 7 days to say whether we can reproduce it.
- We fix it and publish an advisory within 90 days of your report. If a fix needs longer, we
  agree a new date with you before the 90 days are up.
- After 90 days, or once the fix is released if that is sooner, you are free to publish your
  findings. We credit you in the advisory unless you ask us not to.

## Checking a download

Each release lists `SHA256SUMS` beside the exe, a CycloneDX SBOM of what it was built from, and
a GitHub build-provenance attestation. To check that an exe was built from this repository by
its release workflow:

```sh
gh attestation verify cleanshot-X.Y.Z-x64.exe --repo IconRunner05/cleanshot
```
