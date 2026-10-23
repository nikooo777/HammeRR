[← Manual](README.md)

# Transaction history

Inspect user, agent and system edits. Return to an earlier state through Undo/Redo.

1. Click **History…** beside **Undo** and **Redo**.
2. Use **Newer** or **Older** to find a transaction.
3. Select the transaction.
4. Review the required Undo/Redo count.
5. Click **Go to selected state**.

The target is the state immediately after that transaction.
**Initial state** selects the state before the first transaction.
**Cancel selection** clears the target.
**Refresh** reloads the list.

## Limits

- History belongs to the open document. It is not saved in the VMF.
- Navigation supports 256 transactions per jump.
- Apply or cancel active drafts before navigating.
- Changed source state requires a new selection.
- Navigation does not move the camera or save the file.
- A new edit replaces the available Redo branch.
