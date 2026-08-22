# Windows code signing — Azure Artifact Signing runbook

Why this exists: unsigned Windows binaries are hard-blocked on Smart App
Control machines (Copilot+ PCs ship with SAC on) with **no user-facing
bypass**, and SmartScreen warns on every other machine. Signing clears both.
Chosen 2026-08-22: Azure Artifact Signing (Microsoft's managed service) —
$9.99/mo Basic (5,000 signings, 1 profile), keys in Azure's HSM, no cert
files to protect. Signing is wired into `.github/workflows/release.yml`
(secrets-guarded: without the config below, Windows ships unsigned with a
warning — never blocks a release).

The workflow uses `azure/login` (OIDC — the runner's GitHub JWT is exchanged
for an Azure token, no client secret stored) + `azure/artifact-signing-action@v2`,
which signs `engram.exe`, `engramd.exe`, `engramd-mcp.exe` after build and
before packaging, then fails the job unless `Get-AuthenticodeSignature`
reports all three `Valid`. Certificates live ~3 days; the action timestamps
via `http://timestamp.acs.microsoft.com` so signatures outlive the cert.

## One-time setup (portal, ~1 hour + up to ~7 business days validation)

### 1. Azure subscription
Pay-as-you-go is fine. Note the **Subscription ID**.

### 2. Artifact Signing account
Portal: Create a resource → **Artifact Signing** (region: **East US** — the
workflow defaults the endpoint to `eus`; pick another region only if you also
set the `ARTIFACT_SIGNING_ENDPOINT` var to its endpoint from the table below).
SKU: **Basic**. Note the **account name**.

### 3. Identity validation (the long pole — start first)
Inside the signing account → **Identity validation** → New.
- **Individuals: US/Canada only.** First/last name, email (must match the
  Microsoft account email), address exactly as on a government ID → AU10TIX
  photo-ID scan on a phone → Verified ID credential lands in **Microsoft
  Authenticator**. Minutes to ~7 business days; renewed annually.
- Organizations: US/CA/EU/UK, needs 3+ years verifiable tax history.

### 4. Certificate profile
Signing account → **Certificate profiles** → New → type **Public Trust**,
subject = the validated identity. Note the **profile name**.

### 5. Service principal + OIDC federation
- Microsoft Entra ID → **App registrations** → New: `engram-release-signing`.
  Note the **Application (client) ID** and **Directory (tenant) ID**.
- Signing account → **Access control (IAM)** → Add role assignment →
  **Artifact Signing Certificate Profile Signer** → assign to
  `engram-release-signing`.
- App registration → **Certificates & secrets → Federated credentials** →
  add **two**:
  1. Scenario "GitHub Actions deploying Azure resources" → org
     `El-AI-Intelligence`, repo `engram`, entity **Branch**, `main`.
     (Covers `workflow_dispatch` runs.)
  2. Same org/repo, entity type **Other** (custom subject):
     `repo:El-AI-Intelligence/engram:ref:refs/tags/*`
     (Covers tag-push releases — the OIDC subject differs per trigger.)

### 6. GitHub repo config
Settings → Secrets and variables → Actions:
- **Secrets:** `AZURE_CLIENT_ID`, `AZURE_TENANT_ID`, `AZURE_SUBSCRIPTION_ID`
- **Variables:** `ARTIFACT_SIGNING_ACCOUNT` (account name),
  `ARTIFACT_SIGNING_PROFILE` (profile name), and — only if the account is
  NOT East US — `ARTIFACT_SIGNING_ENDPOINT` from:

| Region | Endpoint |
|---|---|
| East US | `https://eus.codesigning.azure.net` |
| West US 2 | `https://wus2.codesigning.azure.net` |
| West US 3 | `https://wus3.codesigning.azure.net` |
| North Europe | `https://neu.codesigning.azure.net` |
| West Europe | `https://weu.codesigning.azure.net` |
| (full table) | Microsoft docs: Artifact Signing → Set up signing integrations |

A region/endpoint mismatch fails signing with a 403 (`SignerSign()` failure).

## Verification

1. Push a tag (or `workflow_dispatch` a release): the Windows build job logs
   should show `Azure login (OIDC)` → `Sign Windows binaries (Artifact
   Signing)` → `Verify Windows signatures` all green.
2. Download an exe from the release → Properties → **Digital Signatures**
   shows the publisher; `Get-AuthenticodeSignature` → `Valid`.
3. On the SAC machine: install and run — no Application Control block, no
   SmartScreen blue dialog (SmartScreen reputation still accrues over the
   first weeks of real downloads).

## Known caveats

- **Not EV**: SmartScreen shows "Unknown publisher" until download reputation
  accrues (typically weeks) — the SAC hard-block is gone immediately, which
  is the point.
- The guard is `AZURE_CLIENT_ID` present ⇒ signing expected. If the var
  exists but the account/profile/role is broken, the release job **fails**
  (better than shipping unsigned silently) — fix the Azure side or clear the
  secret to revert to the unsigned-with-warning mode.
- Identity validation must be renewed annually; changing personal details
  means creating a new validation (no edits).
