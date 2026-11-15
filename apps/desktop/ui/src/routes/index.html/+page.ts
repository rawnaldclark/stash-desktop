// The desktop shell opens the window at `index.html` (and so can anyone, by hand): that is Home.
import { redirect } from '@sveltejs/kit';
import type { PageLoad } from './$types';

export const load: PageLoad = ({ url }) => {
	// The mini player's own window opens at `index.html?view=mini`.
	if (url.searchParams.get('view') === 'mini') redirect(307, '/mini');
	redirect(307, `/${url.search}`);
};
