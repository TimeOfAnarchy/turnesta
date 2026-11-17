# Security policy

## Reporting a vulnerability

Use [GitHub Issues](https://github.com/TimeOfAnarchy/turnesta/issues/new) to contact the maintainer about a suspected vulnerability. Issues in this repository are public.

In the initial issue, include only:

- The affected version, commit or container image digest.
- The affected component and a brief, non-sensitive description of the problem.
- The potential impact in general terms.

Do not publish passwords, API keys, session cookies, access tokens, private keys, database dumps or real client data. Do not attach an exploit, proof of concept or detailed reproduction steps for an unresolved vulnerability to a public issue. Check screenshots and logs for sensitive information before attaching them.

If even the summary would expose sensitive information, open an issue requesting a private reporting channel without describing the vulnerability. Wait for the maintainer to arrange a private channel before sharing technical details. Do not use a public pull request to disclose those details.

For ordinary bugs without a security impact, use the same issue tracker with reproduction steps and expected behavior.

## Versions and investigation

Turnesta is under active development; the current package version is `0.1.0`. Include the commit or image digest because builds can share the same package version.

Reproduce suspected vulnerabilities in a local environment with synthetic data or on a system you are explicitly authorized to test. Do not access another organization's data or disrupt a running service to demonstrate an issue.

Reports are reviewed by the maintainer. This policy does not promise a response deadline, a fix deadline, backports or a bounty. Disclosure of sensitive details should be coordinated after the issue has been assessed and affected deployments can be updated.
