import 'package:flutter/material.dart';

import 'attachment.dart';
import 'bindings/envoix_read.dart';
import 'instrumentation.dart';
import 'labels.dart';

/// UI04 — the evidence the authority recorded, keyed by the session it belongs
/// to. Nothing here is derived: the sequence numbers, the entries and the claim
/// about the timeline's own completeness are all the authority's.
///
/// Copying, sharing and uploading a report are explicit actions and belong to a
/// later step; this screen only inspects.
class LogsScreen extends StatelessWidget {
  const LogsScreen({required this.attachment, super.key});

  final Attachment attachment;

  @override
  Widget build(BuildContext context) {
    final List<EvidenceTimelineView> timelines = attachment.timelines;
    final BuildManifestView? manifest = attachment.build;
    return ListView(
      padding: const EdgeInsets.all(12),
      children: <Widget>[
        if (timelines.isEmpty)
          const Padding(
            padding: EdgeInsets.symmetric(vertical: 24),
            child: Text('No evidence yet.'),
          ),
        for (final EvidenceTimelineView timeline in timelines)
          TimelineCard(timeline: timeline),
        if (manifest != null) BuildCard(manifest: manifest),
      ],
    );
  }
}

/// One session's timeline, and whether it is all of it.
class TimelineCard extends StatelessWidget {
  const TimelineCard({required this.timeline, super.key});

  final EvidenceTimelineView timeline;

  @override
  Widget build(BuildContext context) {
    reportTimeline(timeline);
    final ThemeData theme = Theme.of(context);
    final bool degraded = timeline.status is DiagnosticsStatusViewDegraded;
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: <Widget>[
            Text(
              'card ${timeline.session.card} · '
              'attempt ${timeline.session.generation}',
              style: theme.textTheme.titleMedium,
            ),
            Semantics(
              container: true,
              label: 'Diagnostics',
              value: diagnosticsLabel(timeline.status),
              child: ExcludeSemantics(
                child: Text(
                  diagnosticsLabel(timeline.status),
                  style: TextStyle(
                    color: degraded
                        ? theme.colorScheme.error
                        : theme.colorScheme.onSurface,
                  ),
                ),
              ),
            ),
            const SizedBox(height: 8),
            if (timeline.entries.isEmpty)
              const Text('No entries recorded.')
            else
              for (final TimelineEntryView entry in timeline.entries)
                Text(
                  '${entry.sequence}. ${evidenceLabel(entry.value)}',
                  style: theme.textTheme.bodySmall,
                ),
          ],
        ),
      ),
    );
  }
}

/// What the core this app attached to says it is. The frontend states it; it
/// does not check it — the release gate does that where it can be enforced.
class BuildCard extends StatelessWidget {
  const BuildCard({required this.manifest, super.key});

  final BuildManifestView manifest;

  @override
  Widget build(BuildContext context) {
    reportBuild(manifest);
    final ThemeData theme = Theme.of(context);
    final ProtocolManifestView protocol = manifest.protocol;
    final AbiSchemaManifestView abi = manifest.abiSchema;
    final DeploymentManifestView deployment = manifest.deployment;
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: <Widget>[
            Text('Build', style: theme.textTheme.titleMedium),
            for (final String line in <String>[
              'version ${manifest.packageVersion}',
              'deployment ${deployment.environment}',
              'rendezvous ${deployment.rendezvousEndpoint}',
              'relay ${deployment.relayUrl}',
              'protocol ${protocol.setId} · wire ${protocol.dataWireVersion}',
              'alpn ${protocol.dataAlpn} · magic ${protocol.dataMagic}',
              'read ${abi.readBindingSchemaId} · '
                  'command ${abi.commandBindingSchemaId}',
              'evidence ${abi.evidenceRustAbiId} · '
                  '${abi.evidenceTimelineSchemaId}',
              'receipt ${abi.mailboxReceiptSchemaId} · '
                  'operation ${abi.operationEnvelopeSchemaId}',
            ])
              Text(line, style: theme.textTheme.bodySmall),
          ],
        ),
      ),
    );
  }
}
