import 'package:flutter/material.dart';

import 'attachment.dart';
import 'bindings/envoix_read.dart';
import 'instrumentation.dart';
import 'labels.dart';
import 'theme.dart';

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
      padding: const EdgeInsets.fromLTRB(
        EnvoixSpace.gutter,
        EnvoixSpace.row,
        EnvoixSpace.gutter,
        EnvoixSpace.block,
      ),
      children: <Widget>[
        if (timelines.isEmpty)
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 24),
            child: Text(
              'No evidence yet.',
              style: EnvoixType.body.copyWith(
                color: EnvoixTokens.of(context).muted,
              ),
            ),
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
    final EnvoixTokens tokens = EnvoixTokens.of(context);
    final bool degraded = timeline.status is DiagnosticsStatusViewDegraded;
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(EnvoixSpace.card),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: <Widget>[
            // The session this belongs to, entirely in the machine's own ids.
            Text(
              'card ${timeline.session.card} · '
              'attempt ${timeline.session.generation}',
              style: EnvoixType.monoValue.copyWith(color: tokens.text),
            ),
            Semantics(
              container: true,
              label: 'Diagnostics',
              value: diagnosticsLabel(timeline.status),
              child: ExcludeSemantics(
                child: Text(
                  diagnosticsLabel(timeline.status),
                  style: EnvoixType.value.copyWith(
                    color: degraded ? tokens.danger : tokens.text,
                  ),
                ),
              ),
            ),
            const SizedBox(height: EnvoixSpace.tight),
            _MachineBlock(
              lines: timeline.entries.isEmpty
                  ? const <String>['No entries recorded.']
                  : <String>[
                      for (final TimelineEntryView entry in timeline.entries)
                        '${entry.sequence}. ${evidenceLabel(entry.value)}',
                    ],
            ),
          ],
        ),
      ),
    );
  }
}

/// A run of machine-written lines, sunk into the card that holds them.
///
/// The recess is the design's third level — `surface` for the screen,
/// `surface-raised` for a card, and the page ground for a well inside one — and
/// it is what marks a block as a transcript rather than as prose.
class _MachineBlock extends StatelessWidget {
  const _MachineBlock({required this.lines});

  final List<String> lines;

  @override
  Widget build(BuildContext context) {
    final EnvoixTokens tokens = EnvoixTokens.of(context);
    return Container(
      width: double.infinity,
      padding: const EdgeInsets.all(EnvoixSpace.tight),
      decoration: BoxDecoration(
        color: tokens.bg,
        borderRadius: EnvoixShape.corner,
        border: Border.all(color: tokens.line),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          for (final String line in lines)
            Text(
              line,
              style: EnvoixType.monoLine.copyWith(color: tokens.text),
            ),
        ],
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
    final EnvoixTokens tokens = EnvoixTokens.of(context);
    final ProtocolManifestView protocol = manifest.protocol;
    final AbiSchemaManifestView abi = manifest.abiSchema;
    final DeploymentManifestView deployment = manifest.deployment;
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(EnvoixSpace.card),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: <Widget>[
            Text('Build', style: EnvoixType.title.copyWith(color: tokens.text)),
            const SizedBox(height: EnvoixSpace.tight),
            _MachineBlock(lines: <String>[
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
            ]),
          ],
        ),
      ),
    );
  }
}
