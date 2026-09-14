/-!
# FM-1 — Escrow transfer, authority and rights conservation (SPEC-006 §8, §11, §16)

This is the machine of `models/FM1_Escrow.tla` and `crates/carolina-models/src/fm1.rs`, stated
once more in Lean. The difference is the quantifier: TLC checks the bounded configuration
`Total = 3, Quantities = <<1, 2>>` and the Rust checker enumerates the same finite slice, while
everything below is proved for an **arbitrary** total and an **arbitrary** list of transfer
quantities, of any length.

Mechanising it makes one thing explicit that the bounded runs never had to state: conservation is
not inductive on its own. It needs `Coherent` — a message that has been sent agrees with the phase
that sent it — because `ReceiverInstall` credits the receiver on the strength of `commitSent`
alone, and without `commitSent → donor = committed` a state where the donor never prepared would
create units out of nothing.

Scope, stated plainly: this proves properties of a model. That the Rust implementation refines
this model is an argument, not a theorem, and remains open (see `md/FALTA.md` item 11).
-/

namespace Carolina.Escrow

inductive DonorPhase where
  | absent | prepared | committed | aborted
  deriving DecidableEq, Repr

inductive RecvPhase where
  | absent | accepted | applied | aborted
  deriving DecidableEq, Repr

/-- One transfer: its size, the two phase machines, and the four messages that, once sent, stay
deliverable (the TLA+ model has no channel, only monotone "was sent" flags). -/
structure Transfer where
  q : Nat
  donor : DonorPhase
  recv : RecvPhase
  prepareSent : Bool
  acceptSent : Bool
  commitSent : Bool
  abortSent : Bool
  deriving Repr

def Transfer.fresh (q : Nat) : Transfer :=
  { q := q, donor := .absent, recv := .absent,
    prepareSent := false, acceptSent := false, commitSent := false, abortSent := false }

structure St where
  uDonor : Nat
  uRecv : Nat
  consumed : Nat
  ts : List Transfer
  donorClosed : Bool

/-- The units this transfer is holding in flight: escrowed away from the donor and not yet
credited to the receiver. This is `InTransit` of the TLA+ module. -/
def Transfer.inTransit (t : Transfer) : Nat :=
  if (t.donor = .prepared ∨ t.donor = .committed) ∧ t.recv ≠ .applied then t.q else 0

/-- `X` of the TLA+ module. -/
def St.inFlight (s : St) : Nat := (s.ts.map Transfer.inTransit).sum

def St.init (total : Nat) (qs : List Nat) : St :=
  { uDonor := total, uRecv := 0, consumed := 0,
    ts := qs.map Transfer.fresh, donorClosed := false }

/-- `Conservation` of the TLA+ module: no rights are created or destroyed, only moved between the
donor, the receiver, the consumed pile and the transfers in flight. -/
def Conserved (total : Nat) (s : St) : Prop :=
  total = s.consumed + s.uDonor + s.uRecv + s.inFlight

/-- A sent message agrees with the phase that sent it. Needed to make `Conserved` inductive. -/
def Transfer.Coherent (t : Transfer) : Prop :=
  (t.commitSent = true → t.donor = .committed)
  ∧ (t.abortSent = true → t.donor = .aborted)
  ∧ (t.acceptSent = true → t.recv ≠ .absent)

/-- `CreditImpliesCommit` and `NoRefundAfterCommit`, per transfer. -/
def Transfer.Safe (t : Transfer) : Prop :=
  (t.recv = .applied → t.donor = .committed) ∧ (t.donor = .aborted → t.recv ≠ .applied)

def St.Good (s : St) : Prop := ∀ t ∈ s.ts, t.Coherent ∧ t.Safe

/-- The transition relation. Each per-transfer action is stated on a split `pre ++ t :: post`,
which is the same machine as indexing by `t ∈ Transfers` and is what makes the sum over the list
computable in one `simp`. -/
inductive Step : St → St → Prop where
  | donorPrepare (u r c : Nat) (pre post : List Transfer) (t : Transfer)
      (hq : t.q ≤ u) (hd : t.donor = .absent) :
      Step ⟨u, r, c, pre ++ t :: post, false⟩
           ⟨u - t.q, r, c, pre ++ { t with donor := .prepared } :: post, false⟩
  | sendPrepare (u r c : Nat) (closed : Bool) (pre post : List Transfer) (t : Transfer)
      (hd : t.donor = .prepared) :
      Step ⟨u, r, c, pre ++ t :: post, closed⟩
           ⟨u, r, c, pre ++ { t with prepareSent := true } :: post, closed⟩
  | receiverAccept (u r c : Nat) (closed : Bool) (pre post : List Transfer) (t : Transfer)
      (hs : t.prepareSent = true) (hr : t.recv = .absent) :
      Step ⟨u, r, c, pre ++ t :: post, closed⟩
           ⟨u, r, c, pre ++ { t with recv := .accepted } :: post, closed⟩
  | sendAccept (u r c : Nat) (closed : Bool) (pre post : List Transfer) (t : Transfer)
      (hr : t.recv = .accepted) :
      Step ⟨u, r, c, pre ++ t :: post, closed⟩
           ⟨u, r, c, pre ++ { t with acceptSent := true } :: post, closed⟩
  | donorCommit (u r c : Nat) (closed : Bool) (pre post : List Transfer) (t : Transfer)
      (hs : t.acceptSent = true) (hd : t.donor = .prepared) :
      Step ⟨u, r, c, pre ++ t :: post, closed⟩
           ⟨u, r, c, pre ++ { t with donor := .committed } :: post, closed⟩
  | sendCommit (u r c : Nat) (closed : Bool) (pre post : List Transfer) (t : Transfer)
      (hd : t.donor = .committed) :
      Step ⟨u, r, c, pre ++ t :: post, closed⟩
           ⟨u, r, c, pre ++ { t with commitSent := true } :: post, closed⟩
  | donorAbort (u r c : Nat) (closed : Bool) (pre post : List Transfer) (t : Transfer)
      (hd : t.donor = .prepared) :
      Step ⟨u, r, c, pre ++ t :: post, closed⟩
           ⟨u + t.q, r, c, pre ++ { t with donor := .aborted } :: post, closed⟩
  | sendAbort (u r c : Nat) (closed : Bool) (pre post : List Transfer) (t : Transfer)
      (hd : t.donor = .aborted) :
      Step ⟨u, r, c, pre ++ t :: post, closed⟩
           ⟨u, r, c, pre ++ { t with abortSent := true } :: post, closed⟩
  | receiverInstall (u r c : Nat) (closed : Bool) (pre post : List Transfer) (t : Transfer)
      (hs : t.commitSent = true) (hr : t.recv = .accepted) :
      Step ⟨u, r, c, pre ++ t :: post, closed⟩
           ⟨u, r + t.q, c, pre ++ { t with recv := .applied } :: post, closed⟩
  | receiverAbort (u r c : Nat) (closed : Bool) (pre post : List Transfer) (t : Transfer)
      (hs : t.abortSent = true) (hr : t.recv = .absent ∨ t.recv = .accepted) :
      Step ⟨u, r, c, pre ++ t :: post, closed⟩
           ⟨u, r, c, pre ++ { t with recv := .aborted } :: post, closed⟩
  | spendDonor (u r c : Nat) (ts : List Transfer) (hu : 0 < u) :
      Step ⟨u, r, c, ts, false⟩ ⟨u - 1, r, c + 1, ts, false⟩
  | spendReceiver (u r c : Nat) (closed : Bool) (ts : List Transfer) (hr : 0 < r) :
      Step ⟨u, r, c, ts, closed⟩ ⟨u, r - 1, c + 1, ts, closed⟩
  | closeDonor (u r c : Nat) (ts : List Transfer) :
      Step ⟨u, r, c, ts, false⟩ ⟨u, r, c, ts, true⟩

inductive Reachable (total : Nat) (qs : List Nat) : St → Prop where
  | init : Reachable total qs (St.init total qs)
  | step {s s'} : Reachable total qs s → Step s s' → Reachable total qs s'

end Carolina.Escrow

/-! ## Proofs -/

namespace Carolina.Escrow

/-- Core Lean has no `List.sum_append`; mathlib is deliberately not a dependency of this project,
so the two list facts the proofs need are established here. -/
theorem sum_append_nat (a b : List Nat) : (a ++ b).sum = a.sum + b.sum := by
  induction a with
  | nil => simp
  | cons x xs ih => simp [ih, Nat.add_assoc]

theorem inTransit_fresh (q : Nat) : (Transfer.fresh q).inTransit = 0 := by
  simp [Transfer.inTransit, Transfer.fresh]

theorem sum_map_fresh (qs : List Nat) :
    (List.map (Transfer.inTransit ∘ Transfer.fresh) qs).sum = 0 := by
  induction qs with
  | nil => simp
  | cons q rest ih => simp [inTransit_fresh, ih]

theorem inFlight_split (u r c : Nat) (cl : Bool) (pre post : List Transfer) (t : Transfer) :
    St.inFlight ⟨u, r, c, pre ++ t :: post, cl⟩
      = (pre.map Transfer.inTransit).sum + (t.inTransit + (post.map Transfer.inTransit).sum) := by
  simp [St.inFlight, List.map_append, sum_append_nat]

theorem forall_set {P : Transfer → Prop} {pre post : List Transfer} {t t' : Transfer}
    (h : ∀ x ∈ pre ++ t :: post, P x) (ht : P t') : ∀ x ∈ pre ++ t' :: post, P x := by
  intro x hx
  rcases List.mem_append.1 hx with h1 | h2
  · exact h x (List.mem_append.2 (Or.inl h1))
  · rcases List.mem_cons.1 h2 with rfl | h3
    · exact ht
    · exact h x (List.mem_append.2 (Or.inr (List.mem_cons.2 (Or.inr h3))))

theorem mem_mid {pre post : List Transfer} {t : Transfer} : t ∈ pre ++ t :: post := by simp

/-- Initial states are coherent and safe. -/
theorem init_good (total : Nat) (qs : List Nat) : (St.init total qs).Good := by
  intro t ht
  simp [St.init] at ht
  obtain ⟨q, _, rfl⟩ := ht
  exact ⟨by simp [Transfer.Coherent, Transfer.fresh],
         by simp [Transfer.Safe, Transfer.fresh]⟩

/-- Nothing is in flight before anything has been prepared. -/
theorem init_inFlight (total : Nat) (qs : List Nat) : (St.init total qs).inFlight = 0 := by
  simp [St.init, St.inFlight, List.map_map, sum_map_fresh]

theorem init_conserved (total : Nat) (qs : List Nat) : Conserved total (St.init total qs) := by
  unfold Conserved
  rw [init_inFlight]
  simp [St.init]

/-- Coherence and per-transfer safety are preserved by every transition. -/
theorem step_good {s s' : St} (hg : s.Good) (hs : Step s s') : s'.Good := by
  cases hs with
  | donorPrepare u r c pre post t hq hd =>
      obtain ⟨hc, hsafe⟩ := hg t mem_mid
      have hna : t.recv ≠ RecvPhase.applied := by
        intro h; exact absurd (hsafe.1 h) (by simp [hd])
      refine forall_set hg ⟨?_, ?_⟩
      · refine ⟨fun h => absurd (hc.1 h) (by simp [hd]), fun h => absurd (hc.2.1 h) (by simp [hd]),
                fun h => hc.2.2 h⟩
      · exact ⟨fun h => absurd h hna, fun h => by simp at h⟩
  | sendPrepare u r c cl pre post t hd =>
      obtain ⟨hc, hsafe⟩ := hg t mem_mid
      exact forall_set hg ⟨hc, hsafe⟩
  | receiverAccept u r c cl pre post t hsent hr =>
      obtain ⟨hc, hsafe⟩ := hg t mem_mid
      refine forall_set hg ⟨⟨hc.1, hc.2.1, fun _ => by simp⟩, ?_⟩
      exact ⟨fun h => by simp at h, fun h => by simp⟩
  | sendAccept u r c cl pre post t hr =>
      obtain ⟨hc, hsafe⟩ := hg t mem_mid
      exact forall_set hg ⟨⟨hc.1, hc.2.1, fun _ => by simp [hr]⟩, hsafe⟩
  | donorCommit u r c cl pre post t hsent hd =>
      obtain ⟨hc, hsafe⟩ := hg t mem_mid
      refine forall_set hg ⟨⟨fun _ => by simp, fun h => absurd (hc.2.1 h) (by simp [hd]),
                            fun h => hc.2.2 h⟩, ?_⟩
      exact ⟨fun _ => by simp, fun h => by simp at h⟩
  | sendCommit u r c cl pre post t hd =>
      obtain ⟨hc, hsafe⟩ := hg t mem_mid
      exact forall_set hg ⟨⟨fun _ => hd, hc.2.1, hc.2.2⟩, hsafe⟩
  | donorAbort u r c cl pre post t hd =>
      obtain ⟨hc, hsafe⟩ := hg t mem_mid
      have hna : t.recv ≠ RecvPhase.applied := by
        intro h; exact absurd (hsafe.1 h) (by simp [hd])
      refine forall_set hg ⟨⟨fun h => absurd (hc.1 h) (by simp [hd]), fun _ => by simp,
                            fun h => hc.2.2 h⟩, ?_⟩
      exact ⟨fun h => absurd h hna, fun _ => hna⟩
  | sendAbort u r c cl pre post t hd =>
      obtain ⟨hc, hsafe⟩ := hg t mem_mid
      exact forall_set hg ⟨⟨hc.1, fun _ => hd, hc.2.2⟩, hsafe⟩
  | receiverInstall u r c cl pre post t hsent hr =>
      obtain ⟨hc, hsafe⟩ := hg t mem_mid
      have hcom : t.donor = DonorPhase.committed := hc.1 hsent
      refine forall_set hg ⟨⟨fun _ => hcom, fun h => hc.2.1 h, fun _ => by simp⟩, ?_⟩
      exact ⟨fun _ => hcom, fun h => absurd (h ▸ hcom) (by simp)⟩
  | receiverAbort u r c cl pre post t hsent hr =>
      obtain ⟨hc, hsafe⟩ := hg t mem_mid
      have hab : t.donor = DonorPhase.aborted := hc.2.1 hsent
      refine forall_set hg ⟨⟨fun h => hc.1 h, fun _ => hab, fun _ => by simp⟩, ?_⟩
      exact ⟨fun h => by simp at h, fun _ => by simp⟩
  | spendDonor u r c ts hu => exact hg
  | spendReceiver u r c cl ts hr => exact hg
  | closeDonor u r c ts => exact hg

end Carolina.Escrow

namespace Carolina.Escrow

/-- Rights are conserved by every transition: nothing is created and nothing is destroyed, units
only move between the donor, the receiver, the consumed pile and the transfers in flight.

The hypothesis `s.Good` is not decoration. `receiverInstall` credits the receiver on the strength
of `commitSent` alone, and `donorPrepare`/`donorAbort` move units on the strength of a phase; both
need the sent-message and phase facts to know what was in flight before. Conservation alone is not
inductive. -/
theorem step_conserved {total : Nat} {s s' : St}
    (hg : s.Good) (hc : Conserved total s) (hs : Step s s') : Conserved total s' := by
  cases hs with
  | donorPrepare u r c pre post t hq hd =>
      obtain ⟨_, hsafe⟩ := hg t mem_mid
      have hna : t.recv ≠ RecvPhase.applied := fun h => absurd (hsafe.1 h) (by simp [hd])
      have h1 : t.inTransit = 0 := by simp [Transfer.inTransit, hd]
      have h2 : ({t with donor := DonorPhase.prepared} : Transfer).inTransit = t.q := by
        simp [Transfer.inTransit, hna]
      simp only [Conserved, inFlight_split] at hc ⊢
      rw [h1] at hc; rw [h2]; omega
  | sendPrepare u r c cl pre post t hd =>
      simp only [Conserved, inFlight_split, Transfer.inTransit] at hc ⊢; omega
  | receiverAccept u r c cl pre post t hsent hr =>
      have h1 : t.inTransit = ({t with recv := RecvPhase.accepted} : Transfer).inTransit := by
        simp [Transfer.inTransit, hr]
      simp only [Conserved, inFlight_split] at hc ⊢
      rw [← h1]; omega
  | sendAccept u r c cl pre post t hr =>
      simp only [Conserved, inFlight_split, Transfer.inTransit] at hc ⊢; omega
  | donorCommit u r c cl pre post t hsent hd =>
      have h1 : t.inTransit = ({t with donor := DonorPhase.committed} : Transfer).inTransit := by
        simp [Transfer.inTransit, hd]
      simp only [Conserved, inFlight_split] at hc ⊢
      rw [← h1]; omega
  | sendCommit u r c cl pre post t hd =>
      simp only [Conserved, inFlight_split, Transfer.inTransit] at hc ⊢; omega
  | donorAbort u r c cl pre post t hd =>
      obtain ⟨_, hsafe⟩ := hg t mem_mid
      have hna : t.recv ≠ RecvPhase.applied := fun h => absurd (hsafe.1 h) (by simp [hd])
      have h1 : t.inTransit = t.q := by simp [Transfer.inTransit, hd, hna]
      have h2 : ({t with donor := DonorPhase.aborted} : Transfer).inTransit = 0 := by
        simp [Transfer.inTransit]
      simp only [Conserved, inFlight_split] at hc ⊢
      rw [h1] at hc; rw [h2]; omega
  | sendAbort u r c cl pre post t hd =>
      simp only [Conserved, inFlight_split, Transfer.inTransit] at hc ⊢; omega
  | receiverInstall u r c cl pre post t hsent hr =>
      obtain ⟨hco, _⟩ := hg t mem_mid
      have hcom : t.donor = DonorPhase.committed := hco.1 hsent
      have h1 : t.inTransit = t.q := by simp [Transfer.inTransit, hcom, hr]
      have h2 : ({t with recv := RecvPhase.applied} : Transfer).inTransit = 0 := by
        simp [Transfer.inTransit]
      simp only [Conserved, inFlight_split] at hc ⊢
      rw [h1] at hc; rw [h2]; omega
  | receiverAbort u r c cl pre post t hsent hr =>
      obtain ⟨hco, _⟩ := hg t mem_mid
      have hab : t.donor = DonorPhase.aborted := hco.2.1 hsent
      have h1 : t.inTransit = 0 := by simp [Transfer.inTransit, hab]
      have h2 : ({t with recv := RecvPhase.aborted} : Transfer).inTransit = 0 := by
        simp [Transfer.inTransit, hab]
      simp only [Conserved, inFlight_split] at hc ⊢
      rw [h1] at hc; rw [h2]; omega
  -- these three do not touch `ts`, so `inFlight` reduces to the same sum on both sides
  | spendDonor u r c ts hu => simp only [Conserved, St.inFlight] at hc ⊢; omega
  | spendReceiver u r c cl ts hr => simp only [Conserved, St.inFlight] at hc ⊢; omega
  | closeDonor u r c ts => simp only [Conserved, St.inFlight] at hc ⊢; omega

/-- **FM-1, unbounded.** Every reachable state of the escrow machine conserves rights, credits the
receiver only for a committed transfer, and never refunds a donor whose transfer was installed —
for every total and every list of transfer quantities, of any length. -/
theorem safety (total : Nat) (qs : List Nat) {s : St} (h : Reachable total qs s) :
    Conserved total s ∧ s.Good := by
  induction h with
  | init => exact ⟨init_conserved total qs, init_good total qs⟩
  | step _ hstep ih => exact ⟨step_conserved ih.2 ih.1 hstep, step_good ih.2 hstep⟩

/-- `CreditImpliesCommit` of the TLA+ module, for every reachable state. -/
theorem credit_implies_commit (total : Nat) (qs : List Nat) {s : St} (h : Reachable total qs s) :
    ∀ t ∈ s.ts, t.recv = RecvPhase.applied → t.donor = DonorPhase.committed :=
  fun t ht => ((safety total qs h).2 t ht).2.1

/-- `NoRefundAfterCommit` of the TLA+ module, for every reachable state. -/
theorem no_refund_after_commit (total : Nat) (qs : List Nat) {s : St} (h : Reachable total qs s) :
    ∀ t ∈ s.ts, t.donor = DonorPhase.aborted → t.recv ≠ RecvPhase.applied :=
  fun t ht => ((safety total qs h).2 t ht).2.2

end Carolina.Escrow
