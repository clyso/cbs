/*
 * Copyright © 2026 Clyso GmbH
 *
 *  Licensed under the GNU Affero General Public License, Version 3.0 (the "License");
 *  you may not use this file except in compliance with the License.
 *  You may obtain a copy of the License at
 *
 *  https://www.gnu.org/licenses/agpl-3.0.html
 *
 *  Unless required by applicable law or agreed to in writing, software
 *  distributed under the License is distributed on an "AS IS" BASIS,
 *  WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 *  See the License for the specific language governing permissions and
 *  limitations under the License.
 */

import { describe, expect, it } from 'vitest';
import { GeneralHelper } from './generalHelper';

describe('GeneralHelper', () => {
  describe('formatDate', () => {
    it('formats a Date object', () => {
      const result = GeneralHelper.formatDate(new Date(2024, 0, 15)); // Jan 15 2024

      expect(result).toBe('Jan 15, 2024');
    });

    it('formats an ISO date string', () => {
      const result = GeneralHelper.formatDate('2024-06-01T00:00:00.000Z');

      expect(result).toMatch(/Jun 1, 2024/);
    });
  });

  describe('formatDateTime', () => {
    it('includes time components', () => {
      const result = GeneralHelper.formatDateTime(
        new Date(2024, 0, 15, 9, 5, 3),
      );

      expect(result).toMatch(/Jan 15, 2024/);
      expect(result).toMatch(/09:05:03/);
    });

    it('omits timezone by default', () => {
      const result = GeneralHelper.formatDateTime(new Date(2024, 0, 15));

      expect(result).not.toMatch(/GMT|UTC|[A-Z]{3}T/);
    });

    it('includes timezone when hasTimezone is true', () => {
      const result = GeneralHelper.formatDateTime(new Date(2024, 0, 15), true);

      // timezone abbreviation is locale/env dependent; just check something extra is present
      expect(result.length).toBeGreaterThan(
        GeneralHelper.formatDateTime(new Date(2024, 0, 15), false).length,
      );
    });

    it('accepts a string input', () => {
      const result = GeneralHelper.formatDateTime('2024-03-18T12:00:00');

      expect(result).toMatch(/Mar 18, 2024/);
    });
  });
});
