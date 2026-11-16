/** Puts the keyboard in a field as soon as it shows, its words picked so typing replaces them. The `autofocus`
 * attribute does this only while nothing else has the keyboard, and the button that showed the field has it. */
export function focusNow(node: HTMLInputElement): void {
	node.focus();
	node.select();
}
