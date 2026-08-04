/**
 * The home page's frequently-asked questions.
 *
 * One source of truth for the rendered `<dl>` and the `FAQPage` JSON-LD.
 * Structured data describing answers a reader cannot see is a guidelines
 * violation that gets the markup ignored rather than rewarded, and keeping the
 * two in a single list is what makes them impossible to drift apart.
 */

import type { RepoInfo } from '$lib/graphql/operations';

export interface FaqEntry {
  question: string;
  answer: string;
}

export function faqEntries(repo: RepoInfo): FaqEntry[] {
  return [
    {
      question: `How do I add ${repo.title} as a Flatpak remote?`,
      answer: `Run: ${repo.addRemoteCommand}`
    },
    {
      question: 'Do applications update automatically?',
      answer:
        'Yes. Once the remote is added, flatpak update pulls new builds the same way it ' +
        'does for any other remote, because this is an ordinary OSTree repository.'
    },
    {
      question: 'Are the builds signed?',
      answer: repo.gpgSigned
        ? 'Yes. Commits are GPG-signed and the public key is advertised in the ' +
          '.flatpakrepo file, so flatpak verifies signatures on install.'
        : 'No. This server does not sign its commits, so flatpak cannot verify what it ' +
          'installs from here.'
    }
  ];
}
