// The desktop shell opens the window at `index.html` (and so can anyone, by hand): that is Home.
import { redirect } from '@sveltejs/kit';
import type { PageLoad } from './$types';

export const load: PageLoad = ({ url }) => {
	redirect(307, `/${url.search}`);
};
