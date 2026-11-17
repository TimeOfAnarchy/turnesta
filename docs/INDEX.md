# Documentation

Turnesta provides appointment booking and electronic queues, with an explicit mock-only Tips mode. Start with the guide for your role; the interface is currently Russian.

| You want to... | Start here |
| --- | --- |
| Try Turnesta and understand its scope | [Project README](../README.md) |
| Set up a location, schedule and team | [First configuration](GETTING-STARTED.md) |
| Install locally or on a Linux VPS | [Deployment](DEPLOYMENT.md) |
| Manage updates, backups, recovery and retention | [Operations](OPERATIONS.md) |
| Add booking to an existing website | [Booking widget](BOOKINGS-WIDGET.md) |
| Build, change or test the project | [Development](DEVELOPMENT.md) |
| Understand data isolation, commands and jobs | [Architecture](ARCHITECTURE.md) |

## Suggested reading paths

- **Business owner:** first configuration, then widget integration if needed.
- **Self-hosting administrator:** deployment, operations and the first-configuration guide.
- **Contributor:** development, architecture and the [browser test instructions](../e2e/README.md).

The deployment guide includes the checks still required on the target server. Local automated checks do not establish a verified VPS, external SMTP service or business website integration. Tips does not process real money.

## Project policies

- [Security policy and reporting a vulnerability](../SECURITY.md)
- [Changelog](../CHANGELOG.md)
- [AGPL-3.0 license](../LICENSE)
