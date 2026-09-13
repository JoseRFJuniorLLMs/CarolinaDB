--------------------------- MODULE FM2_Decision ---------------------------
(* FM-2 — C5 decision authority, prepare / unique decision / install / publication /    *)
(* completion (SPEC-008 §10, §11, §15). Same machine as crates/carolina-models/src/fm2.rs.*)
(* Rust and bounded TLC checker evidence is recorded in models/README.md.                 *)

EXTENDS Naturals

CONSTANT N                          \* number of participants
Participants == 1..N

VARIABLES
  decision,          \* "Begun" | "Commit" | "Abort" (replicated CAS state)
  everCommit, everAbort, staleDecision,
  leaderEpoch, staleLeaderAlive,
  commitSent, abortSent, publicationSent, completionSent, successReported,
  phase,             \* per participant: "Idle" | "Prepared" | "VotedNo" | "Installed" | "Published" | "Aborted"
  payload, prepareSent, voteSent, installedSent, seenSent, timeoutAborts

vars == << decision, everCommit, everAbort, staleDecision, leaderEpoch, staleLeaderAlive,
           commitSent, abortSent, publicationSent, completionSent, successReported,
           phase, payload, prepareSent, voteSent, installedSent, seenSent, timeoutAborts >>

Init ==
  /\ decision = "Begun" /\ everCommit = FALSE /\ everAbort = FALSE /\ staleDecision = FALSE
  /\ leaderEpoch = 1 /\ staleLeaderAlive = FALSE
  /\ commitSent = FALSE /\ abortSent = FALSE /\ publicationSent = FALSE /\ completionSent = FALSE
  /\ successReported = FALSE
  /\ phase = [p \in Participants |-> "Idle"] /\ payload = [p \in Participants |-> FALSE]
  /\ prepareSent = [p \in Participants |-> FALSE] /\ voteSent = [p \in Participants |-> FALSE]
  /\ installedSent = [p \in Participants |-> FALSE] /\ seenSent = [p \in Participants |-> FALSE]
  /\ timeoutAborts = 0

Unch(except) == UNCHANGED vars   \* helper comment only; actions list explicit UNCHANGED sets

SendPrepare(p) ==
  /\ ~prepareSent[p] /\ prepareSent' = [prepareSent EXCEPT ![p] = TRUE]
  /\ UNCHANGED << decision, everCommit, everAbort, staleDecision, leaderEpoch, staleLeaderAlive, commitSent,
                  abortSent, publicationSent, completionSent, successReported, phase, payload, voteSent,
                  installedSent, seenSent, timeoutAborts >>

VoteYes(p) ==
  /\ prepareSent[p] /\ phase[p] = "Idle"
  /\ phase' = [phase EXCEPT ![p] = "Prepared"] /\ payload' = [payload EXCEPT ![p] = TRUE]
  /\ UNCHANGED << decision, everCommit, everAbort, staleDecision, leaderEpoch, staleLeaderAlive, commitSent,
                  abortSent, publicationSent, completionSent, successReported, prepareSent, voteSent,
                  installedSent, seenSent, timeoutAborts >>

VoteNo(p) ==
  /\ prepareSent[p] /\ phase[p] = "Idle" /\ phase' = [phase EXCEPT ![p] = "VotedNo"]
  /\ UNCHANGED << decision, everCommit, everAbort, staleDecision, leaderEpoch, staleLeaderAlive, commitSent,
                  abortSent, publicationSent, completionSent, successReported, payload, prepareSent, voteSent,
                  installedSent, seenSent, timeoutAborts >>

SendVote(p) ==
  /\ phase[p] \in {"Prepared", "VotedNo"} /\ ~voteSent[p] /\ voteSent' = [voteSent EXCEPT ![p] = TRUE]
  /\ UNCHANGED << decision, everCommit, everAbort, staleDecision, leaderEpoch, staleLeaderAlive, commitSent,
                  abortSent, publicationSent, completionSent, successReported, phase, payload, prepareSent,
                  installedSent, seenSent, timeoutAborts >>

AllYes == \A p \in Participants : voteSent[p] /\ phase[p] \in {"Prepared", "Installed", "Published"}

DecideCommit ==
  /\ decision = "Begun" /\ AllYes
  /\ decision' = "Commit" /\ everCommit' = TRUE
  /\ UNCHANGED << everAbort, staleDecision, leaderEpoch, staleLeaderAlive, commitSent, abortSent, publicationSent,
                  completionSent, successReported, phase, payload, prepareSent, voteSent, installedSent, seenSent,
                  timeoutAborts >>

DecideAbort ==
  /\ decision = "Begun" /\ decision' = "Abort" /\ everAbort' = TRUE
  /\ UNCHANGED << everCommit, staleDecision, leaderEpoch, staleLeaderAlive, commitSent, abortSent, publicationSent,
                  completionSent, successReported, phase, payload, prepareSent, voteSent, installedSent, seenSent,
                  timeoutAborts >>

SendCommit == /\ decision = "Commit" /\ ~commitSent /\ commitSent' = TRUE
              /\ UNCHANGED << decision, everCommit, everAbort, staleDecision, leaderEpoch, staleLeaderAlive, abortSent,
                              publicationSent, completionSent, successReported, phase, payload, prepareSent, voteSent,
                              installedSent, seenSent, timeoutAborts >>

SendAbort == /\ decision = "Abort" /\ ~abortSent /\ abortSent' = TRUE
             /\ UNCHANGED << decision, everCommit, everAbort, staleDecision, leaderEpoch, staleLeaderAlive, commitSent,
                             publicationSent, completionSent, successReported, phase, payload, prepareSent, voteSent,
                             installedSent, seenSent, timeoutAborts >>

Install(p) ==
  /\ commitSent /\ phase[p] = "Prepared" /\ payload[p] /\ phase' = [phase EXCEPT ![p] = "Installed"]
  /\ UNCHANGED << decision, everCommit, everAbort, staleDecision, leaderEpoch, staleLeaderAlive, commitSent, abortSent,
                  publicationSent, completionSent, successReported, payload, prepareSent, voteSent, installedSent,
                  seenSent, timeoutAborts >>

SendInstalled(p) ==
  /\ phase[p] = "Installed" /\ ~installedSent[p] /\ installedSent' = [installedSent EXCEPT ![p] = TRUE]
  /\ UNCHANGED << decision, everCommit, everAbort, staleDecision, leaderEpoch, staleLeaderAlive, commitSent, abortSent,
                  publicationSent, completionSent, successReported, phase, payload, prepareSent, voteSent, seenSent,
                  timeoutAborts >>

IssuePublicationCertificate ==
  /\ decision = "Commit" /\ ~publicationSent /\ \A p \in Participants : installedSent[p]
  /\ publicationSent' = TRUE
  /\ UNCHANGED << decision, everCommit, everAbort, staleDecision, leaderEpoch, staleLeaderAlive, commitSent, abortSent,
                  completionSent, successReported, phase, payload, prepareSent, voteSent, installedSent, seenSent,
                  timeoutAborts >>

Publish(p) ==
  /\ phase[p] = "Installed" /\ publicationSent /\ phase' = [phase EXCEPT ![p] = "Published"]
  /\ UNCHANGED << decision, everCommit, everAbort, staleDecision, leaderEpoch, staleLeaderAlive, commitSent, abortSent,
                  publicationSent, completionSent, successReported, payload, prepareSent, voteSent, installedSent,
                  seenSent, timeoutAborts >>

SendPublishSeen(p) ==
  /\ phase[p] = "Published" /\ ~seenSent[p] /\ seenSent' = [seenSent EXCEPT ![p] = TRUE]
  /\ UNCHANGED << decision, everCommit, everAbort, staleDecision, leaderEpoch, staleLeaderAlive, commitSent, abortSent,
                  publicationSent, completionSent, successReported, phase, payload, prepareSent, voteSent,
                  installedSent, timeoutAborts >>

IssueCompletionCertificate ==
  /\ publicationSent /\ ~completionSent /\ \A p \in Participants : seenSent[p] /\ completionSent' = TRUE
  /\ UNCHANGED << decision, everCommit, everAbort, staleDecision, leaderEpoch, staleLeaderAlive, commitSent, abortSent,
                  publicationSent, successReported, phase, payload, prepareSent, voteSent, installedSent, seenSent,
                  timeoutAborts >>

ReportFinalSuccess ==
  /\ completionSent /\ ~successReported /\ successReported' = TRUE
  /\ UNCHANGED << decision, everCommit, everAbort, staleDecision, leaderEpoch, staleLeaderAlive, commitSent, abortSent,
                  publicationSent, completionSent, phase, payload, prepareSent, voteSent, installedSent, seenSent,
                  timeoutAborts >>

ParticipantAbort(p) ==
  /\ abortSent /\ phase[p] \in {"Idle", "Prepared", "VotedNo"}
  /\ phase' = [phase EXCEPT ![p] = "Aborted"] /\ payload' = [payload EXCEPT ![p] = FALSE]
  /\ UNCHANGED << decision, everCommit, everAbort, staleDecision, leaderEpoch, staleLeaderAlive, commitSent, abortSent,
                  publicationSent, completionSent, successReported, prepareSent, voteSent, installedSent, seenSent,
                  timeoutAborts >>

LeaderFailover ==
  /\ leaderEpoch = 1 /\ leaderEpoch' = 2 /\ staleLeaderAlive' = TRUE
  /\ UNCHANGED << decision, everCommit, everAbort, staleDecision, commitSent, abortSent, publicationSent,
                  completionSent, successReported, phase, payload, prepareSent, voteSent, installedSent, seenSent,
                  timeoutAborts >>

Next ==
  \/ \E p \in Participants : SendPrepare(p) \/ VoteYes(p) \/ VoteNo(p) \/ SendVote(p) \/ Install(p)
                             \/ SendInstalled(p) \/ Publish(p) \/ SendPublishSeen(p) \/ ParticipantAbort(p)
  \/ DecideCommit \/ DecideAbort \/ SendCommit \/ SendAbort \/ IssuePublicationCertificate
  \/ IssueCompletionCertificate \/ ReportFinalSuccess \/ LeaderFailover

Spec == Init /\ [][Next]_vars

OneDecision == ~(everCommit /\ everAbort)
CommitNeedsAllYes == decision = "Commit" => \A p \in Participants : voteSent[p] /\ phase[p] # "VotedNo" /\ phase[p] # "Idle"
NoStaleDecision == ~staleDecision
PublishedOnlyAfterCertificate ==
  (\E p \in Participants : phase[p] = "Published") =>
     decision = "Commit" /\ publicationSent /\ \A q \in Participants : phase[q] \in {"Installed", "Published"}
CertificateNeedsAllInstalled == publicationSent => \A p \in Participants : phase[p] \in {"Installed", "Published"}
SuccessNeedsCompletion == successReported => completionSent
CompletionNeedsAllSeen == completionSent => \A p \in Participants : phase[p] = "Published"
NoTimeoutAbort == timeoutAborts = 0
AbortNeedsDecision == \A p \in Participants : phase[p] = "Aborted" => decision = "Abort"
CommittedPayloadRetained == \A p \in Participants : (decision = "Commit" /\ phase[p] = "Prepared") => payload[p]

Safety == OneDecision /\ CommitNeedsAllYes /\ NoStaleDecision /\ PublishedOnlyAfterCertificate
          /\ CertificateNeedsAllInstalled /\ SuccessNeedsCompletion /\ CompletionNeedsAllSeen
          /\ NoTimeoutAbort /\ AbortNeedsDecision /\ CommittedPayloadRetained
=============================================================================
