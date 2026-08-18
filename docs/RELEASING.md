# Release runbook

No general Auracle release may be promoted until OSS counsel reviews the GPL
process-separation boundary. Approval of CI or a GitHub environment is a
release-control mechanism, not a statement of legal clearance.

## Build, sign, and verify

Set a Developer ID Application identity already installed in the signing
keychain, then run:

```sh
export AURACLE_CODESIGN_IDENTITY='Developer ID Application: Example, Inc. (TEAMID)'
scripts/build-universal.sh
codesign --verify --deep --strict --verbose=2 dist/auracle-imessage-exporter
shasum -a 256 -c dist/auracle-imessage-exporter.sha256
```

The checksum filename is normative because Auracle looks for it beside the
selected executable:

```text
auracle-imessage-exporter.sha256
```

## Notarize

ZIP the signed executable, submit it with App Store Connect API credentials,
and retain the successful notarytool log:

```sh
ditto -c -k --keepParent dist/auracle-imessage-exporter \
  dist/auracle-imessage-exporter.zip
xcrun notarytool submit dist/auracle-imessage-exporter.zip \
  --key /secure/AuthKey_KEYID.p8 --key-id KEYID --issuer ISSUER_UUID --wait
spctl --assess --type execute --verbose=2 dist/auracle-imessage-exporter
```

A raw executable and ZIP cannot carry a stapled ticket. Gatekeeper retrieves
the ticket online; the adjacent Developer ID signature remains verifiable
offline. If distribution packaging changes to a supported container, staple
and validate that container before publishing.

## Publish

The `Release` workflow performs the same build, signature, checksum,
notarization, and GitHub release steps using the `oss-counsel-approved`
environment. Repository administrators must configure that environment with
the appropriate protection rules and signing/notarization secrets before the
workflow is used. Every release note must include:

> The GPL process-separation boundary still requires OSS counsel review before
> general Auracle distribution. This release is not a claim of legal clearance.

Release assets must include the executable and its adjacent checksum. Keep
this repository's tags and updater lifecycle independent from Auracle.

## Clean-Mac acceptance

1. Download both assets from the GitHub release on a clean Mac.
2. Verify the checksum, Developer ID signature, architecture list, and
   Gatekeeper assessment.
3. Place both files together and select the executable from Auracle's “Locate
   exporter…” step.
4. Grant Auracle Full Disk Access and sign in with a tester account.
5. Run a full import against the public API. Verify batch acknowledgements and
   that the local cursor advances only after `/complete`.
6. Confirm API records contain no attachment content or filesystem paths.
7. Run again and verify the pass is incremental and idempotent.

Steps 3–7 require the signed Auracle UI, tester credentials, and public API;
they cannot be certified by repository CI alone.
